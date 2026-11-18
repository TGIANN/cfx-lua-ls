//! LuaLS's rules about one file's doc comments: `duplicate-doc-field`,
//! `duplicate-doc-param`, `undefined-doc-param`, `doc-field-no-class`,
//! `unknown-cast-variable`, `unknown-diag-code`, `unknown-operator`.

use super::Linter;
use crate::luacats::{DocGroup, DocTag, DocType, Key, Tag, view};
use crate::names::DeclKind;
use crate::{TextRange, TextSize};
use cfx_lua_syntax::SyntaxKind::*;
use cfx_lua_syntax::SyntaxNode;
use std::collections::HashMap;

/// LuaLS's diagnostic codes (`script/proto/diagnostic.lua`).
const LUALS_CODES: &[&str] = &[
    "ambiguity-1", "assign-type-mismatch", "await-in-sync", "cast-local-type", "cast-type-mismatch", "circle-doc-class", "close-non-object",
    "code-after-break", "codestyle-check", "count-down-loop", "deprecated", "different-requires", "discard-returns", "doc-field-no-class",
    "duplicate-doc-alias", "duplicate-doc-field", "duplicate-doc-param", "duplicate-index", "duplicate-set-field", "empty-block",
    "global-element", "global-in-nil-env", "incomplete-signature-doc", "inject-field", "invisible", "lowercase-global", "missing-fields",
    "missing-global-doc", "missing-local-export-doc", "missing-parameter", "missing-return", "missing-return-value", "name-style-check",
    "need-check-nil", "newfield-call", "newline-call", "no-unknown", "not-yieldable", "param-type-mismatch", "redefined-local",
    "redundant-parameter", "redundant-return", "redundant-return-value", "redundant-value", "return-type-mismatch", "spell-check",
    "trailing-space", "unbalanced-assignments", "undefined-doc-class", "undefined-doc-name", "undefined-doc-param", "undefined-env-child",
    "undefined-field", "undefined-global", "unknown-cast-variable", "unknown-diag-code", "unknown-operator", "unnecessary-assert",
    "unreachable-code", "unused-function", "unused-label", "unused-local", "unused-vararg",
];

/// LuaLS's syntax error codes (`parser/compile.lua`, `parser/luadoc.lua`), which
/// `---@diagnostic` may also name.
const LUALS_SYNTAX_CODES: &[&str] = &[
    "action-after-return", "ambiguous-syntax", "args-after-dots", "assign-const-global", "block-after-else", "break-outside",
    "declare-const", "env-is-global", "err-assign-as-eq", "err-comment-prefix", "err-c-long-comment", "err-do-as-then", "err-eq-as-assign",
    "err-esc", "err-nonstandard-symbol", "err-then-as-do", "exp-in-action", "global-close-attribute", "index-in-func-name",
    "jump-local-scope", "keyword", "local-limit", "luadoc-error-diag-mode", "luadoc-miss-alias-extends", "luadoc-miss-alias-name",
    "luadoc-miss-arg-name", "luadoc-miss-cate-name", "luadoc-miss-class-extends-name", "luadoc-miss-class-name", "luadoc-miss-diag-mode",
    "luadoc-miss-diag-name", "luadoc-miss-field-extends", "luadoc-miss-field-name", "luadoc-miss-fun-after-overload",
    "luadoc-miss-generic-name", "luadoc-miss-local-name", "luadoc-miss-module-name", "luadoc-miss-operator-name",
    "luadoc-miss-param-extends", "luadoc-miss-param-name", "luadoc-miss-see-name", "luadoc-miss-sign-name", "luadoc-miss-symbol",
    "luadoc-miss-type-name", "luadoc-miss-vararg-type", "luadoc-miss-version", "lua-doc-miss-sign", "malformed-number", "miss-end",
    "miss-esc-x", "miss-exp", "miss-exponent", "miss-field", "miss-loop-max", "miss-loop-min", "miss-method", "miss-name",
    "miss-sep-in-table", "miss-space-between", "miss-symbol", "multi-close", "need-paren", "nesting-long-mark", "no-visible-label",
    "redefined-label", "set-const", "unexpect-dots", "unexpect-efunc-name", "unexpect-gfunc-name", "unexpect-lfunc-name",
    "unexpect-symbol", "unicode-name", "unknown-attribute", "unknown-symbol", "unsupport-named-vararg", "unsupport-symbol",
    "variable-not-declared",
];

/// `---@operator` names LuaLS knows (`vm/operator.lua`).
const OPERATORS: &[&str] =
    &["add", "band", "bnot", "bor", "bxor", "call", "concat", "div", "idiv", "len", "mod", "mul", "pow", "sar", "shl", "shr", "sub", "unm"];

pub(super) fn run(l: &mut Linter) {
    if !l.text.contains("---") {
        return;
    }
    let groups = super::code_doc_groups(&l.root, l.text);
    // Class -> (field key, its range) over the whole file.
    let mut fields: HashMap<String, Vec<(String, TextRange)>> = HashMap::new();
    for g in &groups {
        group(l, g, &mut fields);
    }
    // duplicate-doc-alias: an alias declared twice, or named like a class.
    let mut aliases: Vec<(&str, TextRange)> = Vec::new();
    let mut classes: Vec<&str> = Vec::new();
    for t in groups.iter().flat_map(|g| &g.tags) {
        match &t.tag {
            Tag::Alias { name, .. } => aliases.push((name, t.name.unwrap_or(t.range))),
            Tag::Class { name, .. } => classes.push(name),
            _ => {}
        }
    }
    for (name, range) in &aliases {
        if classes.contains(name) || aliases.iter().filter(|(n, _)| n == name).count() > 1 {
            l.push("duplicate-doc-alias", *range, format!("Duplicate defined alias `{name}`."));
        }
    }
    for list in fields.values() {
        for (key, range) in list {
            if list.iter().filter(|(k, _)| k == key).count() > 1 {
                l.push("duplicate-doc-field", *range, format!("Duplicate defined fields `{key}`."));
            }
        }
    }
    diagnostic_codes(l);
}

fn group(l: &mut Linter, g: &DocGroup, fields: &mut HashMap<String, Vec<(String, TextRange)>>) {
    let mut class: Option<&str> = None;
    for t in &g.tags {
        match &t.tag {
            Tag::Class { name, .. } => class = Some(name),
            Tag::Field { key, ty, .. } => match class {
                None => {
                    let start = t.name.map_or(t.range.start(), |n| n.start());
                    l.push("doc-field-no-class", trim_end(l.text, TextRange::new(start, t.range.end())), "The field must be defined after the class.");
                }
                Some(c) => {
                    // Several `fun` fields of one name are overloads (LuaLS).
                    if matches!(ty, DocType::Fun(_)) {
                        continue;
                    }
                    let (key, range) = match key {
                        Key::Name(n) => (n.clone(), t.name.unwrap_or(t.range)),
                        Key::Index(k) => (format!("[{}]", view(k)), index_range(l.text, t)),
                    };
                    fields.entry(c.to_string()).or_default().push((key, range));
                }
            },
            Tag::Cast { name, ops } => {
                unknown_cast(l, t, name);
                cast_mismatch(l, t, name, ops);
            }
            Tag::Operator { op, .. } if !OPERATORS.contains(&op.as_str()) => {
                if let Some(r) = word_in(l.text, t.range, op) {
                    l.push("unknown-operator", r, format!("Unknown operator `{op}`."));
                }
            }
            _ => {}
        }
    }
    params(l, g);
    returns(l, g);
}

/// The function a doc group sits on: the first one starting in the statement
/// or table field after it.
fn bound_function(l: &Linter, g: &DocGroup) -> Option<SyntaxNode> {
    let (item, from) = bound_item(l, g.end)?;
    item.descendants().find(|d| d.kind() == FUNC_BODY && d.text_range().start() >= from)
}

/// A return the caller can count on: not optional, not nil-able, not `any`.
fn required(r: &crate::luacats::FunRet) -> bool {
    fn nil_able(t: &DocType) -> bool {
        match t {
            DocType::Optional(_) => true,
            DocType::Name(n) => matches!(n.as_str(), "nil" | "any" | "unknown" | "void"),
            DocType::Union(ps) => ps.iter().any(nil_able),
            _ => false,
        }
    }
    !r.optional && !nil_able(&r.ty) && !vararg(r)
}

/// `---@return ...`, `---@return ...T`, `---@return T ...`.
fn vararg(r: &crate::luacats::FunRet) -> bool {
    matches!(r.ty, DocType::Dots) || r.name.as_deref() == Some("...")
}

/// missing-return, missing-return-value and redundant-return-value for the
/// function the group sits on, against its `---@return` lines (or a
/// `---@type fun(): R` on it).
fn returns(l: &mut Linter, g: &DocGroup) {
    let mut rets: Vec<crate::luacats::FunRet> = Vec::new();
    let mut declared = false;
    for t in &g.tags {
        match &t.tag {
            Tag::Return { rets: r, .. } => {
                declared = true;
                rets.extend(r.iter().cloned());
            }
            Tag::Type { tys } if tys.len() == 1 => {
                if let DocType::Fun(f) = &tys[0] {
                    declared = true;
                    rets.extend(f.rets.iter().cloned());
                }
            }
            _ => {}
        }
    }
    if !declared {
        return;
    }
    let Some(body) = bound_function(l, g) else { return };
    let Some(block) = body.children().find(|c| c.kind() == BLOCK) else { return };
    // An empty body declares the function (`---@meta` files, ox_lib's `cache`
    // stub filled in later); LuaLS skips these only in meta files.
    if block.children().all(|s| s.kind() == EMPTY_STAT) {
        return;
    }
    // `---@return ...` (or a `...` last) takes any number.
    let unbounded = rets.last().is_some_and(vararg);
    let max = if unbounded { usize::MAX } else { rets.len() };
    let min = rets.iter().rposition(required).map_or(0, |i| i + 1);
    if min > 0 && !returns_always(&block) {
        let stats: Vec<SyntaxNode> = block.children().filter(|s| s.kind() != EMPTY_STAT).collect();
        let at = match stats.last().and_then(crate::util::trimmed) {
            Some(r) => r.end(),
            None => body.children_with_tokens().find(|e| e.kind() == END_KW).map_or(body.text_range().end(), |e| e.text_range().start()),
        };
        l.push("missing-return", TextRange::empty(at), "Annotations specify that a return value is required here.");
    }
    for ret in own_returns(&block) {
        let values: Vec<SyntaxNode> = ret.children().find(|c| c.kind() == EXPR_LIST).map(|v| v.children().collect()).unwrap_or_default();
        // A call or `...` last returns an unknown number of values.
        let open = values.last().is_some_and(|v| matches!(v.kind(), CALL_EXPR | METHOD_CALL_EXPR | VARARG_EXPR));
        let count = values.len();
        // `---@overload`s may return other types.
        let overloaded = g.tags.iter().any(|t| matches!(t.tag, Tag::Overload(..)));
        // Literals and constructors only: the flow inference may miss a
        // narrowing on a name (`if type(v) ~= 'table' then return v end`).
        let checked = values.iter().zip(&rets).filter(|(v, _)| !overloaded && matches!(v.kind(), LITERAL_EXPR | TABLE_EXPR | FUNCTION_EXPR));
        for (v, r) in checked {
            if let Some((got, want)) = l.types().value_mismatch(v, &r.ty) {
                let index = values.iter().position(|x| x == v).map_or(1, |i| i + 1);
                let msg = format!("Annotations specify that return value #{index} has a type of `{want}`, returning value of type `{got}` here instead.");
                l.push("return-type-mismatch", v.text_range(), msg);
            }
        }
        // `return 1, f()` with `f`'s own `---@return` lines: its values take
        // the positions from the call on.
        let last_call = values.last().filter(|_| open && !overloaded).and_then(|v| Some((v, l.types().call_doc_returns(v)?)));
        if let Some((call, theirs)) = &last_call {
            for (j, r) in theirs.iter().enumerate() {
                let Some(want) = rets.get(count - 1 + j) else { break };
                if vararg(r) || vararg(want) {
                    break;
                }
                if let Some((got, want)) = l.types().type_mismatch(&r.ty, &want.ty) {
                    let msg = format!("Annotations specify that return value #{} has a type of `{want}`, returning value of type `{got}` here instead.", count + j);
                    l.push("return-type-mismatch", call.text_range(), msg);
                    break;
                }
            }
            let at_least = count - 1 + theirs.iter().rposition(required).map_or(0, |i| i + 1);
            if count <= max && at_least > max && !theirs.iter().any(vararg) {
                let msg = format!("Annotations specify that at most {max} return value(s) are required, found {at_least} returned here instead.");
                l.push("redundant-return-value", call.text_range(), msg);
            }
        }
        if count < min
            && !open
            && let Some(kw) = ret.children_with_tokens().find(|e| e.kind() == RETURN_KW)
        {
            let msg = format!("Annotations specify that at least {min} return value(s) are required, found {count} returned here instead.");
            l.push("missing-return-value", kw.text_range(), msg);
        }
        if count > max {
            let msg = format!("Annotations specify that at most {max} return value(s) are required, found {count} returned here instead.");
            for v in &values[max..] {
                // A call last may return nothing.
                if open && v == values.last().unwrap() && max + 1 == count {
                    continue;
                }
                l.push("redundant-return-value", v.text_range(), msg.clone());
            }
        }
    }
}

/// The `return` statements of a function body, not of functions inside it.
fn own_returns(block: &SyntaxNode) -> Vec<SyntaxNode> {
    let mut out = Vec::new();
    let mut stack = vec![block.clone()];
    while let Some(n) = stack.pop() {
        for c in n.children() {
            match c.kind() {
                RETURN_STAT => out.push(c),
                FUNCTION_EXPR | FUNCTION_STAT | LOCAL_FUNCTION_STAT => {}
                _ => stack.push(c),
            }
        }
    }
    out
}

/// Whether control never falls off the end of a block, as LuaLS judges it: a
/// `return` or `error()` / `os.exit()` in the block (or in a loop or `do`
/// body inside it, not crossing an `if`), an `if` whose every branch with an
/// `else` returns, or a loop whose body does, or a `while` on a true literal.
fn returns_always(block: &SyntaxNode) -> bool {
    block.children().any(|s| match s.kind() {
        RETURN_STAT => true,
        CALL_STAT => s.first_child().and_then(|c| c.first_child()).is_some_and(|callee| {
            let t = callee.text().to_string();
            t == "error" || t == "os.exit"
        }),
        IF_STAT => {
            let blocks: Vec<SyntaxNode> = std::iter::once(s.clone())
                .chain(s.children().filter(|c| matches!(c.kind(), ELSEIF_CLAUSE | ELSE_CLAUSE)))
                .filter_map(|c| c.children().find(|b| b.kind() == BLOCK))
                .collect();
            s.children().any(|c| c.kind() == ELSE_CLAUSE) && blocks.iter().all(returns_always)
        }
        WHILE_STAT => {
            let cond = s.children().find(|c| c.kind() != BLOCK);
            cond.is_some_and(|c| c.kind() == LITERAL_EXPR && c.first_token().is_some_and(|t| !matches!(t.kind(), FALSE_KW | NIL_KW)))
                || s.children().find(|b| b.kind() == BLOCK).is_some_and(|b| returns_always(&b) || returns_inside(&b))
        }
        DO_STAT | REPEAT_STAT | NUMERIC_FOR_STAT | GENERIC_FOR_STAT => {
            s.children().find(|b| b.kind() == BLOCK).is_some_and(|b| returns_always(&b) || returns_inside(&b))
        }
        _ => false,
    })
}

/// A `return` directly in a loop or `do` body (LuaLS counts it for the
/// enclosing function: its parser marks the innermost `if` branch or function).
fn returns_inside(block: &SyntaxNode) -> bool {
    block.children().any(|s| match s.kind() {
        RETURN_STAT => true,
        DO_STAT | WHILE_STAT | REPEAT_STAT | NUMERIC_FOR_STAT | GENERIC_FOR_STAT => s.children().find(|b| b.kind() == BLOCK).is_some_and(|b| returns_inside(&b)),
        _ => false,
    })
}

/// The type inside `---@field [T]`.
fn index_range(text: &str, t: &DocTag) -> TextRange {
    let Some(n) = t.name else { return t.range };
    let line = &text[TextRange::new(n.start(), t.range.end())];
    match line.find(']') {
        Some(close) if close > 1 => {
            let inner = &line[1..close];
            let lead = inner.len() - inner.trim_start().len();
            TextRange::at(n.start() + TextSize::from((1 + lead) as u32), TextSize::of(inner.trim()))
        }
        _ => n,
    }
}

fn trim_end(text: &str, r: TextRange) -> TextRange {
    let s = &text[r];
    TextRange::at(r.start(), TextSize::of(s.trim_end()))
}

/// `word` as a whole word in the range.
fn word_in(text: &str, r: TextRange, word: &str) -> Option<TextRange> {
    let s = &text[r];
    let is_word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    s.match_indices(word)
        .find(|(i, _)| !is_word(s[..*i].chars().next_back()) && !is_word(s[i + word.len()..].chars().next()))
        .map(|(i, _)| TextRange::at(r.start() + TextSize::from(i as u32), TextSize::of(word)))
}

/// duplicate-doc-param and undefined-doc-param for one group.
fn params(l: &mut Linter, g: &DocGroup) {
    let docs: Vec<(&str, TextRange)> = g
        .tags
        .iter()
        .filter_map(|t| match &t.tag {
            Tag::Param { name, .. } => Some((name.as_str(), t.name.unwrap_or(t.range))),
            _ => None,
        })
        .collect();
    if docs.is_empty() {
        return;
    }
    for (name, range) in &docs {
        if docs.iter().filter(|(n, _)| n == name).count() > 1 {
            l.push("duplicate-doc-param", *range, format!("Duplicate params `{name}`."));
        }
    }
    // The function the group binds: the first one starting in the statement or
    // table field after it. Without one the params describe nothing.
    let bound = bound_item(l, g.end);
    let body = bound.as_ref().and_then(|(item, from)| item.descendants().find(|d| d.kind() == FUNC_BODY && d.text_range().start() >= *from));
    let params: Vec<String> = body
        .as_ref()
        .and_then(|b| b.children().find(|c| c.kind() == PARAM_LIST))
        .map(|p| l.src(p.text_range()).trim_matches(['(', ')']).split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect())
        .unwrap_or_default();
    // `...` takes any name.
    if params.iter().any(|p| p == "...") {
        return;
    }
    let method = body.as_ref().and_then(|b| b.parent()).is_some_and(|f| f.children().any(|c| c.kind() == FUNC_NAME && l.src(c.text_range()).contains(':')));
    for (name, range) in &docs {
        let known = params.iter().any(|p| p == name) || (*name == "self" && method);
        if !known {
            l.push("undefined-doc-param", *range, format!("Undefined param `{name}`."));
        }
    }
}

/// The statement or table field starting right after `end` (a doc group's last
/// line), and where its code starts.
fn bound_item(l: &Linter, end: TextSize) -> Option<(SyntaxNode, TextSize)> {
    let mut tok = l.root.token_at_offset(end).right_biased()?;
    while tok.kind().is_trivia() {
        tok = tok.next_token()?;
    }
    let from = tok.text_range().start();
    if from < end {
        return None;
    }
    // A blank line between the docs and the code ends the binding (LuaLS); a
    // plain `--` comment line does not.
    let gap: Vec<&str> = l.text[TextRange::new(end, from)].split('\n').collect();
    if gap.len() > 2 && gap[1..gap.len() - 1].iter().any(|line| line.trim().is_empty()) {
        return None;
    }
    let item = tok
        .parent_ancestors()
        .find(|a| matches!(a.kind(), FIELD_NAMED | FIELD_INDEXED | FIELD_POS | FIELD_SET) || a.parent().is_some_and(|p| p.kind() == BLOCK))?;
    Some((item, from))
}

/// cast-type-mismatch: `---@cast x T` (no `+` / `-`) on a local whose doc
/// comment declares a type `T` does not fit.
fn cast_mismatch(l: &mut Linter, t: &DocTag, name: &str, ops: &[(char, crate::luacats::DocType)]) {
    let at = t.range.start();
    if !ops.iter().any(|(op, _)| *op == '=') {
        return;
    }
    let n = l.names;
    let Some(d) = n
        .decls
        .iter()
        .enumerate()
        .filter(|(_, d)| d.name == name && matches!(d.kind, DeclKind::Local | DeclKind::Param) && d.range.end() <= at && scope_of(l, d.range).is_some_and(|s| s.contains(at)))
        .max_by_key(|(_, d)| d.range.start())
        .map(|(i, _)| i)
    else {
        return;
    };
    let line = &l.text[t.range];
    let Some(after) = t.name.map(|r| usize::from(r.end() - t.range.start())).or_else(|| line.find(name).map(|i| i + name.len())) else { return };
    for (op, ty) in ops {
        if *op != '=' {
            continue;
        }
        let Some((def, want)) = l.types().cast_mismatch(d, ty) else { continue };
        // The type's text: the first one after the name (one cast per tag is the usual case).
        let rest = &line[after..];
        let lead = rest.len() - rest.trim_start().len();
        let body = rest.trim_start();
        let len = body.find([',', '#']).unwrap_or(body.len());
        let body = body[..len].trim_end();
        let start = t.range.start() + crate::TextSize::from((after + lead) as u32);
        let range = crate::TextRange::at(start, crate::TextSize::from(body.len() as u32));
        l.push("cast-type-mismatch", range, format!("Cannot convert `{def}` to `{want}`."));
        return;
    }
}

/// unknown-cast-variable: `---@cast x T` where no local `x` is visible and no
/// global `x` is known.
fn unknown_cast(l: &mut Linter, t: &DocTag, name: &str) {
    let head = name.split(['.', ':', '[']).next().unwrap_or(name);
    if head.is_empty() || head == "self" {
        return;
    }
    let at = t.range.start();
    let n = l.names;
    let visible = n.decls.iter().any(|d| {
        d.name == head
            && matches!(d.kind, DeclKind::Local | DeclKind::LocalFunction | DeclKind::Param | DeclKind::ForVar)
            && d.range.end() <= at
            && scope_of(l, d.range).is_some_and(|s| s.contains(at))
    });
    if visible {
        return;
    }
    let lib = crate::library::library();
    let global = l.ctx.globals_unknown
        || n.refs.iter().any(|r| r.target == crate::names::Target::Global && l.src(r.range) == head)
        || lib.has_global(head, l.ctx.game)
        || (l.ctx.defined_elsewhere)(head)
        || l.ctx.config.globals.contains(head);
    if global {
        return;
    }
    if let Some(r) = word_in(l.text, t.range, head) {
        l.push("unknown-cast-variable", r, format!("Unknown type conversion variable `{head}`."));
    }
}

/// Where a local is visible, generously: the node around the block it is
/// declared in (the function, `do`, loop or file), so a doc comment at the end
/// of the block still counts.
pub(super) fn scope_of(l: &Linter, decl: TextRange) -> Option<TextRange> {
    let node = l.root.covering_element(decl);
    let start = match node {
        rowan::NodeOrToken::Node(n) => n,
        rowan::NodeOrToken::Token(t) => t.parent()?,
    };
    let scope = start.ancestors().find(|a| matches!(a.kind(), BLOCK | FUNC_BODY | NUMERIC_FOR_STAT | GENERIC_FOR_STAT))?;
    let outer = match scope.kind() {
        BLOCK => scope.parent().unwrap_or(scope),
        _ => scope,
    };
    Some(outer.text_range())
}

/// unknown-diag-code: a code in `---@diagnostic disable: a, b` that neither LuaLS
/// nor this server has and that is one or two letters away from one of theirs
/// (a typo). Unlike LuaLS, other codes are left alone: servers run other linters
/// too (Qbox's scripts carry `unused-argument`, `qbox/prefer-cache`).
fn diagnostic_codes(l: &mut Linter) {
    let mut found = Vec::new();
    for t in l.root.descendants_with_tokens().filter_map(|e| e.into_token()) {
        if t.kind() != COMMENT {
            continue;
        }
        let text = t.text();
        let Some(rest) = text.strip_prefix("---@diagnostic") else { continue };
        let Some(colon) = rest.find(':') else { continue };
        let mut at = t.text_range().start() + TextSize::from(("---@diagnostic".len() + colon + 1) as u32);
        for part in rest[colon + 1..].split(',') {
            // Codes end where a description starts (`-- why`, `# why`, words).
            let code = part.split(" --").next().unwrap_or(part).split('#').next().unwrap_or(part).trim();
            if !code.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '/' | '_')) {
                break;
            }
            let lead = part.len() - part.trim_start().len();
            if !code.is_empty()
                && !known_code(code)
                && let Some(near) = nearest_code(code)
            {
                found.push((TextRange::at(at + TextSize::from(lead as u32), TextSize::of(code)), code.to_string(), near));
            }
            at += TextSize::of(part) + TextSize::from(1);
        }
    }
    for (r, code, near) in found {
        l.push("unknown-diag-code", r, format!("Unknown diagnostic code `{code}`. Did you mean `{near}`?"));
    }
}

fn all_codes() -> impl Iterator<Item = &'static str> {
    LUALS_CODES.iter().chain(LUALS_SYNTAX_CODES).copied().chain(super::RULES.iter().map(|(c, _)| *c))
}

fn known_code(code: &str) -> bool {
    all_codes().any(|c| c == code)
}

/// A known code at most two edits away.
fn nearest_code(code: &str) -> Option<&'static str> {
    all_codes().map(|c| (edits(code, c), c)).filter(|(d, _)| *d <= 2).min_by_key(|(d, _)| *d).map(|(_, c)| c)
}

/// Levenshtein distance.
fn edits(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != *cb)).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[b.len()]
}
