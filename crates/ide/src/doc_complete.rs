//! Completion inside `---` doc comments (LuaLS `core/completion/completion.lua`,
//! `tryluaDocCate` / `tryluaDocBySource` / `tryluaDocOfFunction`) and LuaLS's
//! fuzzy name match (`core/matchkey.lua`).

use crate::assist::{Completion, CompletionKind, FileCtx};
use crate::names::{DeclKind, Names};
use crate::selection::token_at;
use crate::{Parse, TextRange, TextSize};
use cfx_lua_syntax::SyntaxKind::*;
use cfx_lua_syntax::SyntaxNode;
use std::collections::BTreeSet;

/// LuaLS's match of typed text against a name: a prefix (any case), or every
/// typed letter found in the name with the first at its start or at a word
/// boundary (`zac` matches `zabc`).
pub(crate) fn matchkey(input: &str, other: &str) -> bool {
    if input == other || input.is_empty() {
        return true;
    }
    if input.len() > other.len() {
        return false;
    }
    valid_first(input, other) && almost_same(input, other)
}

fn valid_first(input: &str, other: &str) -> bool {
    let (Some(first), Some(o0)) = (input.chars().next(), other.chars().next()) else { return false };
    let up = first.to_ascii_uppercase();
    if up == o0.to_ascii_uppercase() {
        return true;
    }
    let ob: Vec<char> = other.chars().collect();
    let pos = ob.iter().skip(1).position(|&c| c == up).map(|p| p + 1);
    let Some(pos) = pos else {
        // A word after a symbol (`a_b` for `B`).
        return up.is_ascii_uppercase() && ob.windows(2).skip(1).any(|w| !w[0].is_ascii_alphabetic() && w[1] == first.to_ascii_lowercase());
    };
    let c = ob[pos];
    if !c.is_ascii_uppercase() {
        return true;
    }
    !ob[pos - 1].is_ascii_uppercase()
}

fn almost_same(input: &str, other: &str) -> bool {
    let (me, ot) = (input.to_lowercase(), other.to_lowercase());
    if ot.starts_with(&me) {
        return true;
    }
    let mut counts = std::collections::HashMap::new();
    for c in ot.chars() {
        *counts.entry(c).or_insert(0usize) += 1;
    }
    me.chars().all(|c| match counts.get_mut(&c) {
        Some(n) if *n > 0 => {
            *n -= 1;
            true
        }
        _ => false,
    })
}

const DOC_TAGS: &[&str] = &[
    "class", "type", "alias", "param", "return", "field", "generic", "vararg", "overload", "deprecated", "meta", "version", "see",
    "diagnostic", "module", "async", "nodiscard", "cast", "operator", "source", "enum", "package", "private", "protected",
];

const DIAGNOSTIC_MODES: &[&str] = &["disable-next-line", "disable-line", "disable", "enable"];

const OPERATORS: &[&str] = &[
    "unm", "bnot", "len", "add", "sub", "mul", "div", "mod", "pow", "idiv", "band", "bor", "bxor", "shl", "shr", "concat", "call",
];

fn item(label: impl Into<String>, kind: CompletionKind) -> Completion {
    Completion { label: label.into(), kind, detail: None, deprecated: false, replace: None }
}

/// Completion when `offset` is inside a `---` comment: None elsewhere, and in a
/// doc comment where nothing applies.
pub(crate) fn complete(parse: &Parse, names: &Names, text: &str, offset: TextSize, cx: FileCtx) -> Option<Vec<Completion>> {
    let root = parse.syntax();
    let tok = token_at(&root, offset).left_biased().filter(|t| t.kind() == COMMENT && t.text_range().start() < offset)?;
    let start = usize::from(tok.text_range().start());
    let line = &text[start..usize::from(offset)];
    let after = line.strip_prefix("---")?;
    // `---` / `--- ` above a function: its doc lines.
    if after.trim().is_empty() {
        return function_doc(&root, offset).map(|c| vec![c]);
    }
    let body = after.trim_start().strip_prefix('@')?;
    let tag_len = body.bytes().take_while(|b| b.is_ascii_alphanumeric() || *b == b'_').count();
    let (tag, rest) = body.split_at(tag_len);
    if rest.is_empty() {
        return Some(DOC_TAGS.iter().filter(|t| matchkey(tag, t)).map(|t| item(*t, CompletionKind::Event)).collect());
    }
    // The word being typed at the end of the line (`A.B` included).
    let word_len = rest.bytes().rev().take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.')).count();
    let word = &rest[rest.len() - word_len..];
    let before = rest[..rest.len() - word_len].trim_start();
    let word_range = TextRange::new(offset - TextSize::from(word_len as u32), offset);
    let types = |exclude: Option<&str>| type_names(text, cx, word, word_range, exclude);
    match tag {
        "class" => {
            // `---@class B : A, Z|`: the parents.
            let own = before.split(|c: char| c == ':' || c.is_whitespace()).find(|s| !s.is_empty()).unwrap_or("");
            before.contains(':').then(|| types(Some(own)).into_iter().filter(|c| c.kind == CompletionKind::Class && class_name(text, cx, &c.label)).collect())
        }
        "type" | "return" | "see" | "vararg" | "overload" => Some(types(None)),
        "param" | "field" | "alias" | "generic" if !before.is_empty() => Some(types(None)),
        "param" => Some(param_names(&root, offset, word)),
        "cast" if before.is_empty() => Some(visible_locals(parse, names, offset, word)),
        "operator" if before.is_empty() => Some(OPERATORS.iter().filter(|o| matchkey(word, o)).map(|o| item(*o, CompletionKind::Operator)).collect()),
        "diagnostic" => {
            if before.contains(':') {
                let codes: BTreeSet<&str> = crate::lint::diagnostic_names().collect();
                Some(codes.into_iter().filter(|c| matchkey(word, c)).map(|c| item(c, CompletionKind::Value)).collect())
            } else {
                Some(DIAGNOSTIC_MODES.iter().filter(|m| matchkey(word, m)).map(|m| item(*m, CompletionKind::Enum)).collect())
            }
        }
        _ => None,
    }
}

/// Whether `name` is a class (not an alias or enum) for `---@class X: name`.
fn class_name(text: &str, cx: FileCtx, name: &str) -> bool {
    crate::api::doc_classes(text).iter().any(|c| c.name == name)
        || crate::library::classes().contains_key(name)
        || cx.ws.is_some_and(|(ws, _)| ws.has_class(name))
}

/// Class, alias and enum names: the file's, the workspace's and the library's.
fn type_names(text: &str, cx: FileCtx, word: &str, range: TextRange, exclude: Option<&str>) -> Vec<Completion> {
    let mut all: BTreeSet<String> = BTreeSet::new();
    all.extend(crate::api::doc_classes(text).into_iter().map(|c| c.name));
    for line in text.lines() {
        let t = line.trim_start();
        for tag in ["---@alias", "---@enum"] {
            if let Some(rest) = t.strip_prefix(tag).filter(|r| r.starts_with([' ', '\t'])) {
                let rest = rest.trim_start();
                // `---@enum (key) Name`.
                let rest = if rest.starts_with('(') { rest.split_once(')').map_or("", |(_, r)| r.trim_start()) } else { rest };
                let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '-')).collect();
                if !name.is_empty() {
                    all.insert(name);
                }
            }
        }
    }
    all.extend(crate::library::classes().keys().cloned());
    if let Some((ws, _)) = cx.ws {
        all.extend(ws.doc_type_names());
    }
    all.into_iter()
        .filter(|n| Some(n.as_str()) != exclude && matchkey(word, n))
        .map(|n| {
            let replace = n.contains(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).then_some(range);
            Completion { label: n, kind: CompletionKind::Class, detail: None, deprecated: false, replace }
        })
        .collect()
}

/// The function a doc block sits on: the first one starting after `offset`.
fn next_function(root: &SyntaxNode, offset: TextSize) -> Option<SyntaxNode> {
    root.descendants().filter(|n| n.kind() == FUNC_BODY).find(|b| b.text_range().start() > offset)
}

/// The parameter names of a function body (`self` first for `function a:b()`).
fn params(body: &SyntaxNode) -> Vec<String> {
    let mut out = Vec::new();
    let method = body.parent().is_some_and(|h| {
        h.kind() == FUNCTION_STAT && h.children().find(|c| c.kind() == FUNC_NAME).is_some_and(|n| n.children_with_tokens().any(|t| t.kind() == COLON))
    });
    if method {
        out.push("self".to_string());
    }
    for p in body.children().filter(|c| c.kind() == PARAM_LIST).flat_map(|l| l.children().filter(|p| p.kind() == PARAM).collect::<Vec<_>>()) {
        if let Some(t) = p.first_token() {
            out.push(t.text().to_string());
        }
    }
    out
}

/// `---@param |`: the names of the function below, and all of them at once.
fn param_names(root: &SyntaxNode, offset: TextSize, word: &str) -> Vec<Completion> {
    let Some(body) = next_function(root, offset) else { return Vec::new() };
    let names = params(&body);
    let mut out: Vec<Completion> = names.iter().filter(|n| matchkey(word, n)).map(|n| item(n.clone(), CompletionKind::Interface)).collect();
    let own: Vec<&String> = names.iter().filter(|n| *n != "self").collect();
    if word.is_empty() && own.len() > 1 {
        out.insert(0, item(own.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "), CompletionKind::Snippet));
    }
    out
}

/// The `---@param` names written above the function of a parameter list
/// (`function f(|)` under `---@param xyz T`), and all of them at once.
pub(crate) fn documented_params(text: &str, list: &SyntaxNode, word: &str) -> Vec<Completion> {
    let Some(stat) = list.ancestors().find(|a| matches!(a.kind(), FUNCTION_STAT | LOCAL_FUNCTION_STAT | LOCAL_STAT | ASSIGN_STAT)) else { return Vec::new() };
    let mut names: Vec<String> = Vec::new();
    for t in stat.children_with_tokens().filter_map(|e| e.into_token()).take_while(|t| t.kind().is_trivia()) {
        if t.kind() != COMMENT {
            continue;
        }
        let line = &text[t.text_range()];
        if let Some(rest) = line.strip_prefix("---@param").filter(|r| r.starts_with([' ', '\t'])) {
            let name: String = rest.trim_start().chars().take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '.')).collect();
            if !name.is_empty() && !names.contains(&name) {
                names.push(name);
            }
        }
    }
    let mut out: Vec<Completion> = names.iter().filter(|n| matchkey(word, n)).map(|n| item(n.clone(), CompletionKind::Interface)).collect();
    if names.len() > 1 {
        out.insert(0, item(names.join(", "), CompletionKind::Snippet));
    }
    out
}

/// `---@cast |`: the locals seen here.
fn visible_locals(parse: &Parse, names: &Names, offset: TextSize, word: &str) -> Vec<Completion> {
    let mut seen = BTreeSet::new();
    names
        .decls
        .iter()
        .filter(|d| d.kind != DeclKind::Label && d.range.end() <= offset && matchkey(word, &d.name))
        // A comment at the end of the file lies after the file's block.
        .filter(|d| crate::assist::scope(parse, d).is_some_and(|s| s.contains_inclusive(offset) || (s.start() == TextSize::from(0) && s.end() <= offset)))
        .filter(|d| seen.insert(d.name.clone()))
        .map(|d| item(d.name.clone(), CompletionKind::Local))
        .collect()
}

/// `---|` above a function: `---@param` lines for its parameters and a
/// `---@return`, as one snippet.
fn function_doc(root: &SyntaxNode, offset: TextSize) -> Option<Completion> {
    let body = next_function(root, offset)?;
    // Only the function right below the comment.
    let holder = body.parent()?;
    let stat = holder.ancestors().find(|a| matches!(a.kind(), FUNCTION_STAT | LOCAL_FUNCTION_STAT | LOCAL_STAT | ASSIGN_STAT))?;
    if !stat.text_range().contains(offset) && stat.text_range().start() < offset {
        return None;
    }
    let mut lines: Vec<String> = params(&body).into_iter().filter(|p| p != "self").map(|p| format!("@param {p} any")).collect();
    let returns = body.descendants().any(|n| {
        n.kind() == RETURN_STAT && n.ancestors().find(|a| a.kind() == FUNC_BODY).as_ref() == Some(&body) && n.children().any(|c| c.kind() == EXPR_LIST)
    });
    if returns {
        lines.push("@return any".to_string());
    }
    if lines.is_empty() {
        return None;
    }
    let mut c = item(lines.join("\n---"), CompletionKind::Snippet);
    c.detail = Some("doc comment".to_string());
    Some(c)
}
