//! LuaLS definition, references, highlight and rename cases, extracted to
//! `data/luals_names.json` by `tools/extract_luals_names.py`. Cases that need
//! fields, types or other files wait in `SKIP` with the reason; a skipped case that
//! starts passing fails the test, so the list stays true.

use cfx_lua_ide::workspace::{TypeTarget, Workspace};
use cfx_lua_ide::{TextRange, TextSize, field_references, highlights, names, parse, rename};
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The case's text as the one file of a workspace, and what is at `at`.
struct Typed {
    root: std::path::PathBuf,
    file: std::path::PathBuf,
    ws: Workspace,
    target: Option<TypeTarget>,
}

impl Typed {
    fn new(text: &str, at: TextSize) -> Typed {
        static N: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!("cfx-names-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("a.lua");
        std::fs::write(&file, text).unwrap();
        let ws = Workspace::scan(std::slice::from_ref(&root));
        let target = ws.type_target(&file, text, at);
        Typed { root, file, ws, target }
    }

    fn read(p: &std::path::Path) -> Option<String> {
        std::fs::read_to_string(p).ok()
    }

    fn references(&self) -> Vec<TextRange> {
        let Some(t) = &self.target else { return Vec::new() };
        self.ws.type_references(&self.file, t, &Self::read).into_iter().flat_map(|(_, r)| r).collect()
    }

    fn rename(&self, new: &str) -> Option<Result<Vec<TextRange>, String>> {
        let t = self.target.as_ref()?;
        Some(self.ws.type_rename(&self.file, t, new, &Self::read).map(|f| f.into_iter().flat_map(|(_, r)| r).collect()))
    }
}

impl Drop for Typed {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// `(suite, file, n, reason)`.
const SKIP: &[(&str, &str, usize, &str)] = &[
    ("definition", "label", 5, "duplicate label: a compile error in Lua 5.4"),
    ("definition", "set", 6, "`global` declarations are Lua 5.5"),
    ("references", "all", 2, "the second value a function returns: the index follows the first"),
    ("references", "all", 5, "case commented out in LuaLS"),
    ("references", "all", 6, "function values through returns: LuaCATS types (phase 5)"),
    ("references", "all", 7, "function values through returns: LuaCATS types (phase 5)"),
    ("references", "all", 9, "a `---@generic T` return typed from the argument: not followed by the index"),
    ("references", "all", 10, "a `---@generic T` return typed from the argument: not followed by the index"),
    ("references", "common", 18, "`self` references with the object's declaration (LuaLS-specific)"),
    ("references", "common", 23, "`_G.x` as the global `x`"),
    ("references", "common", 29, "case commented out in LuaLS"),
    ("rename", "init", 3, "a field of a global: rename refuses, other files may use it"),
    ("rename", "init", 6, "new name that is not a Lua name (LuaLS writes `_ENV[...]` or a field path)"),
    ("rename", "init", 7, "new name that is not a Lua name (LuaLS writes `_ENV[...]` or a field path)"),
    ("rename", "init", 8, "new name that is not a Lua name (LuaLS writes `_ENV[...]` or a field path)"),
    ("rename", "init", 9, "new name that is not a Lua name (LuaLS writes `_ENV[...]` or a field path)"),
    ("rename", "init", 10, "new name that is not a Lua name (LuaLS writes `_ENV[...]` or a field path)"),
    ("rename", "init", 11, "new name that is not a Lua name (LuaLS writes `_ENV[...]` or a field path)"),
    ("rename", "init", 12, "new name that is not a Lua name (LuaLS writes `_ENV[...]` or a field path)"),
    ("rename", "init", 13, "new name that is not a Lua name (LuaLS writes `_ENV[...]` or a field path)"),
    ("rename", "init", 14, "new name that is not a Lua name (LuaLS writes `_ENV[...]` or a field path)"),
];

/// The script without its markers and the marked ranges per marker character.
fn catch(script: &str) -> (String, Vec<(char, TextRange)>) {
    let b = script.as_bytes();
    let mut out = String::new();
    let mut open: Vec<(char, usize)> = Vec::new();
    let mut marks = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'<' && i + 1 < b.len() && b"!?~".contains(&b[i + 1]) {
            open.push((b[i + 1] as char, out.len()));
            i += 2;
            continue;
        }
        if b"!?~".contains(&b[i]) && b.get(i + 1) == Some(&b'>') && open.last().is_some_and(|(c, _)| *c == b[i] as char) {
            let (c, start) = open.pop().unwrap();
            marks.push((c, TextRange::new(TextSize::from(start as u32), TextSize::from(out.len() as u32))));
            i += 2;
            continue;
        }
        let ch = script[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    (out, marks)
}

fn sorted(mut v: Vec<TextRange>) -> Vec<TextRange> {
    v.sort_by_key(|r| (r.start(), r.end()));
    v.dedup();
    v
}

fn of(marks: &[(char, TextRange)], kinds: &str) -> Vec<TextRange> {
    marks.iter().filter(|(c, _)| kinds.contains(*c)).map(|&(_, r)| r).collect()
}

/// `None` when the case passes, else what went wrong.
fn run(case: &Value) -> Option<String> {
    let suite = case["suite"].as_str().unwrap();
    if suite == "rename" {
        let (old, new) = (case["old"].as_str().unwrap(), case["new"].as_str().unwrap());
        let before = case["before"].as_str().unwrap();
        let after = case["after"].as_str().unwrap();
        // LuaLS: the first `old` with a non-name character on each side.
        let b = before.as_bytes();
        let is_word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
        let at = (1..b.len()).find(|&i| {
            before[i..].starts_with(old) && !is_word(b[i - 1]) && b.get(i + old.len()).is_some_and(|&c| !is_word(c))
        })?;
        let p = parse(before);
        let n = names(&p, before);
        let at = TextSize::from(at as u32);
        let mut text = before.to_string();
        // As the server: a class, alias or field by type, else names and paths.
        let ranges = match Typed::new(before, at).rename(new) {
            Some(r) => r,
            None => rename(&p, &n, before, at, new),
        };
        if let Ok(mut ranges) = ranges {
            ranges.sort_by_key(|r| std::cmp::Reverse(r.start()));
            for r in ranges {
                text.replace_range(std::ops::Range::<usize>::from(r), new);
            }
        }
        return (text != after).then(|| format!("got:\n{text}\nexpected:\n{after}"));
    }
    let (text, marks) = catch(case["script"].as_str().unwrap());
    let p = parse(&text);
    let n = names(&p, &text);
    match suite {
        "definition" => {
            let Some(&input) = of(&marks, "?").first() else { return Some(format!("no input mark: {text}")) };
            let got = sorted(n.definition(&text, input.start()));
            let want = sorted(of(&marks, "!"));
            (got != want).then(|| format!("{text}\ngot {got:?}, expected {want:?}"))
        }
        "references" => {
            let Some(&input) = of(&marks, "?~").first() else { return Some(format!("no input mark: {text}")) };
            let mut got = n.references(&text, input.start());
            if got.is_empty() {
                got = Typed::new(&text, input.start()).references();
            }
            if got.is_empty() {
                got = field_references(&p, &n, input.start()).unwrap_or_default().into_iter().map(|(r, _)| r).collect();
            }
            let got = sorted(got);
            let want = sorted(of(&marks, "!~"));
            (got != want).then(|| format!("{text}\ngot {got:?}, expected {want:?}"))
        }
        "highlight" => {
            let want = sorted(of(&marks, "!"));
            for r in &want {
                let mid = r.start() + TextSize::from(u32::from(r.len()) / 2);
                let got = sorted(highlights(&p, &n, &text, mid).into_iter().map(|(r, _)| r).collect());
                if got != want {
                    return Some(format!("{text}\nat {mid:?}: got {got:?}, expected {want:?}"));
                }
            }
            None
        }
        _ => unreachable!(),
    }
}

#[test]
fn luals_name_cases() {
    let data: Value = serde_json::from_str(include_str!("data/luals_names.json")).unwrap();
    let mut errors = Vec::new();
    let (mut passed, mut skipped) = (0, 0);
    for case in data.as_array().unwrap() {
        let key = (case["suite"].as_str().unwrap(), case["file"].as_str().unwrap(), case["n"].as_u64().unwrap() as usize);
        let skip = SKIP.iter().any(|&(s, f, n, _)| (s, f, n) == key);
        match (run(case), skip) {
            (None, false) => passed += 1,
            (Some(e), false) => errors.push(format!("{key:?}: {e}")),
            (Some(_), true) => skipped += 1,
            (None, true) => errors.push(format!("{key:?} passes now: take it off SKIP")),
        }
    }
    println!("{passed} passed, {skipped} skipped");
    if let Ok(path) = std::env::var("NAMES_FAILS") {
        std::fs::write(path, errors.join("\n\n")).unwrap();
    }
    assert!(errors.is_empty(), "{} failing:\n\n{}", errors.len(), errors.join("\n\n"));
}
