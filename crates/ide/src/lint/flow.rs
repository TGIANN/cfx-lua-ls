//! LuaLS's rules about control flow and dead code: `unreachable-code`,
//! `redundant-return`, `unused-function`, and `ambiguity-1`.

use super::{Linter, Tag};
use crate::TextRange;
use crate::names::{Access, Attrib, DeclKind, Target};
use crate::util::trimmed;
use cfx_lua_syntax::SyntaxKind::{self, *};
use cfx_lua_syntax::SyntaxNode;
use std::collections::HashMap;

pub(super) fn node(l: &mut Linter, n: &SyntaxNode) {
    match n.kind() {
        BLOCK if n.parent().is_some_and(|p| matches!(p.kind(), FUNC_BODY | SOURCE_FILE)) => unreachable(l, n),
        RETURN_STAT => redundant_return(l, n),
        BINARY_EXPR => ambiguity(l, n),
        TABLE_EXPR => {
            newfield_call(l, n);
            for (range, got, want) in l.types().field_mismatches(n) {
                l.push("assign-type-mismatch", range, format!("Cannot assign `{got}` to `{want}`."));
            }
            if let Some((class, missing)) = l.types().missing_fields(n) {
                let list = missing.iter().map(|m| format!("`{m}`")).collect::<Vec<_>>().join(", ");
                l.push("missing-fields", n.text_range(), format!("Missing required fields in type `{class}`: {list}"));
            }
        }
        ASSIGN_STAT => {
            for (range, code, msg) in l.types().assign_mismatches(n) {
                l.push(code, range, msg);
            }
        }
        ARG_LIST => {
            for arg in n.children() {
                if let Some((got, want)) = l.types().param_mismatch(&arg) {
                    l.push("param-type-mismatch", arg.text_range(), format!("Cannot assign `{got}` to parameter `{want}`."));
                }
            }
        }
        LOCAL_STAT => {
            close_non_object(l, n);
            for (range, got, want) in l.types().local_mismatches(n) {
                l.push("assign-type-mismatch", range, format!("Cannot assign `{got}` to `{want}`."));
            }
        }
        NAME_EXPR if l.types().need_check_nil(n) => l.push("need-check-nil", n.text_range(), "Need check nil.".to_string()),
        _ => {}
    }
}

/// newfield-call: a call in a table constructor whose arguments start on a later
/// line than the function (`{ print\n'a' }` is `print('a')`, not two fields).
fn newfield_call(l: &mut Linter, table: &SyntaxNode) {
    for field in table.children().filter(|f| f.kind() == FIELD_POS) {
        let Some(call) = field.first_child().filter(|c| c.kind() == CALL_EXPR) else { continue };
        let (Some(func), Some(args)) = (call.first_child(), call.children().find(|c| c.kind() == ARG_LIST)) else { continue };
        let between = &l.text[TextRange::new(func.text_range().end(), args.text_range().start())];
        if between.contains('\n') {
            let msg = format!("Will be interpreted as `{}{}`. It may be necessary to add a `,` or `;`.", l.src(func.text_range()), l.src(args.text_range()));
            l.push("newfield-call", call.text_range(), msg);
        }
    }
}

/// close-non-object: `local x <close> = v` where `v` is a number, string or
/// boolean (or there is no value).
fn close_non_object(l: &mut Linter, stat: &SyntaxNode) {
    let names: Vec<SyntaxNode> = stat.children().filter(|c| c.kind() == LOCAL_NAME).collect();
    let values: Vec<SyntaxNode> = stat.children().find(|c| c.kind() == EXPR_LIST).map(|v| v.children().collect()).unwrap_or_default();
    for (i, name) in names.iter().enumerate() {
        if !l.src(name.text_range()).contains("<close>") && !name.children_with_tokens().any(|e| e.kind() == ATTRIB && l.src(e.text_range()).contains("close")) {
            continue;
        }
        let Some(tok) = name.children_with_tokens().find(|e| e.kind() == NAME) else { continue };
        let Some(value) = values.get(i) else {
            l.push("close-non-object", tok.text_range(), "Cannot close a value of this type. (Unless set `__close` meta method)");
            continue;
        };
        let Some(d) = l.names.decls.iter().position(|d| d.range == tok.text_range()) else { continue };
        let ty = l.types().decl(d);
        if ty.as_deref().is_some_and(|t| matches!(t, "integer" | "number" | "string" | "boolean" | "true" | "false") || t.starts_with('\'') || t.starts_with('"')) {
            l.push("close-non-object", value.text_range(), "Cannot close a value of this type. (Unless set `__close` meta method)");
        }
    }
}

/// duplicate-set-field: one function field of one object defined twice in the
/// same branch of the file (`function M.f()` twice). Different `if` branches,
/// functions, and other files are left alone.
pub(super) fn duplicate_set_fields(l: &mut Linter) {
    if l.text.starts_with("---@meta") {
        return;
    }
    // (object, field) -> [(name range, top block)]
    let mut sets: HashMap<(String, String), Vec<(TextRange, TextRange)>> = HashMap::new();
    let root = l.root.clone();
    for stat in root.descendants().filter(|s| matches!(s.kind(), FUNCTION_STAT | ASSIGN_STAT)) {
        let target = match stat.kind() {
            FUNCTION_STAT => {
                let Some(name) = stat.children().find(|c| c.kind() == FUNC_NAME) else { continue };
                let text = l.src(name.text_range());
                let parts: Vec<&str> = text.split(['.', ':']).map(str::trim).collect();
                if parts.len() != 2 {
                    continue;
                }
                let Some(first) = name.first_token() else { continue };
                (object_key(l, first.text_range()), parts[1].to_string(), name.text_range())
            }
            _ => {
                let vars: Vec<SyntaxNode> = stat.children().find(|c| c.kind() == VAR_LIST).map(|v| v.children().collect()).unwrap_or_default();
                let values: Vec<SyntaxNode> = stat.children().find(|c| c.kind() == EXPR_LIST).map(|v| v.children().collect()).unwrap_or_default();
                let (Some(var), Some(value), 1) = (vars.first(), values.first(), vars.len()) else { continue };
                if value.kind() != FUNCTION_EXPR || var.kind() != INDEX_EXPR {
                    continue;
                }
                let Some(obj) = var.first_child().filter(|o| o.kind() == NAME_EXPR) else { continue };
                let Some(field) = super::field_name(var) else { continue };
                let Some(first) = obj.first_token() else { continue };
                (object_key(l, first.text_range()), field.text().to_string(), var.text_range())
            }
        };
        let (Some(obj), field, range) = target else { continue };
        sets.entry((obj, field)).or_default().push((range, top_block(&stat)));
    }
    let mut found: Vec<(TextRange, String)> = Vec::new();
    for ((_, field), list) in &sets {
        for (r, top) in list {
            if list.iter().any(|(r2, top2)| r2 != r && top2 == top) {
                found.push((*r, field.clone()));
            }
        }
    }
    found.sort_by_key(|(r, _)| r.start());
    for (r, field) in found {
        l.push("duplicate-set-field", r, format!("Duplicate field `{field}`."));
    }
}

/// A name that identifies the object: the local it refers to, or the global.
fn object_key(l: &Linter, name: TextRange) -> Option<String> {
    let n = l.names;
    let i = n.refs.binary_search_by_key(&name.start(), |r| r.range.start()).ok()?;
    Some(match n.refs[i].target {
        Target::Decl(d) => format!("local {d}"),
        Target::Global => format!("global {}", l.src(name)),
    })
}

/// The innermost `if` branch, function or file around a statement (LuaLS's
/// `getTopFunctionOfIf`).
fn top_block(stat: &SyntaxNode) -> TextRange {
    stat.ancestors()
        .skip(1)
        .find(|a| match a.kind() {
            BLOCK => a.parent().is_some_and(|p| matches!(p.kind(), IF_STAT | ELSEIF_CLAUSE | ELSE_CLAUSE | FUNC_BODY | SOURCE_FILE)),
            _ => false,
        })
        .map_or(stat.text_range(), |b| b.text_range())
}

/// The statements of a block, without `;`.
fn stats(block: &SyntaxNode) -> Vec<SyntaxNode> {
    block.children().filter(|s| s.kind() != EMPTY_STAT).collect()
}

/// unreachable-code: the statements of a function (or the file) after an `if`
/// whose every branch returns, a `do` block that returns, or a `while true`
/// loop that never breaks. Unlike LuaLS, a `return` inside a `for` or `while`
/// loop does not count (the loop may not run).
fn unreachable(l: &mut Linter, block: &SyntaxNode) {
    let stats = stats(block);
    let Some(i) = stats.iter().position(exits) else { return };
    if i + 1 >= stats.len() {
        return;
    }
    if let (Some(a), Some(b)) = (trimmed(&stats[i + 1]), trimmed(stats.last().unwrap())) {
        l.push_full("unreachable-code", TextRange::new(a.start(), b.end()), "Unreachable code.".into(), Some(Tag::Unnecessary), None);
    }
}

/// Whether a block statement never lets control reach the next statement.
fn exits(stat: &SyntaxNode) -> bool {
    match stat.kind() {
        IF_STAT => {
            let blocks: Vec<SyntaxNode> = std::iter::once(stat.clone())
                .chain(stat.children().filter(|c| matches!(c.kind(), ELSEIF_CLAUSE | ELSE_CLAUSE)))
                .filter_map(|c| c.children().find(|b| b.kind() == BLOCK))
                .collect();
            stat.children().any(|c| c.kind() == ELSE_CLAUSE) && blocks.iter().all(block_exits)
        }
        DO_STAT | REPEAT_STAT => stat.children().find(|b| b.kind() == BLOCK).is_some_and(|b| block_exits(&b)),
        WHILE_STAT => {
            let cond = stat.children().find(|c| c.kind() != BLOCK);
            let body = stat.children().find(|c| c.kind() == BLOCK);
            cond.is_some_and(|c| always_true(&c)) && body.is_some_and(|b| !breaks(&b))
        }
        _ => false,
    }
}

/// A block that returns, calls `error` / `os.exit` at its own level, or holds a
/// statement that exits.
fn block_exits(block: &SyntaxNode) -> bool {
    stats(block).iter().any(|s| match s.kind() {
        RETURN_STAT => true,
        CALL_STAT => s.first_child().and_then(|c| c.first_child()).is_some_and(|callee| {
            let t = callee.text().to_string();
            t == "error" || t == "os.exit"
        }),
        _ => exits(s),
    })
}

/// `true`, a non-zero number or string, or `not false`, built only from literals.
fn always_true(cond: &SyntaxNode) -> bool {
    match cond.kind() {
        LITERAL_EXPR => cond.first_token().is_some_and(|t| !matches!(t.kind(), FALSE_KW | NIL_KW)),
        PAREN_EXPR => cond.children().next().is_some_and(|c| always_true(&c)),
        UNARY_EXPR => {
            let op = cond.children_with_tokens().find_map(|e| e.into_token().filter(|t| t.kind() == NOT_KW));
            op.is_some() && cond.children().next().is_some_and(|c| always_false(&c))
        }
        _ => false,
    }
}

fn always_false(cond: &SyntaxNode) -> bool {
    match cond.kind() {
        LITERAL_EXPR => cond.first_token().is_some_and(|t| matches!(t.kind(), FALSE_KW | NIL_KW)),
        PAREN_EXPR => cond.children().next().is_some_and(|c| always_false(&c)),
        _ => false,
    }
}

/// A `break` or `goto` leaving the loop whose body this is (not one of an inner
/// loop or function).
fn breaks(body: &SyntaxNode) -> bool {
    let mut stack = vec![body.clone()];
    while let Some(n) = stack.pop() {
        for c in n.children() {
            match c.kind() {
                BREAK_STAT | GOTO_STAT => return true,
                WHILE_STAT | REPEAT_STAT | NUMERIC_FOR_STAT | GENERIC_FOR_STAT => {
                    // A `goto` in an inner loop may still leave this one.
                    if c.descendants().any(|d| d.kind() == GOTO_STAT) {
                        return true;
                    }
                }
                FUNCTION_EXPR | FUNC_BODY | FUNCTION_STAT | LOCAL_FUNCTION_STAT => {}
                _ => stack.push(c),
            }
        }
    }
    false
}

/// redundant-return: a `return` without values that ends a function.
fn redundant_return(l: &mut Linter, ret: &SyntaxNode) {
    if ret.children().any(|c| c.kind() == EXPR_LIST) {
        return;
    }
    if !ret.parent().is_some_and(|b| b.kind() == BLOCK && b.parent().is_some_and(|f| f.kind() == FUNC_BODY)) {
        return;
    }
    if let Some(r) = trimmed(ret) {
        l.push_full("redundant-return", r, "Redundant return.".into(), Some(Tag::Unnecessary), None);
    }
}

fn binary_op(n: &SyntaxNode) -> Option<SyntaxKind> {
    if n.kind() != BINARY_EXPR {
        return None;
    }
    n.children_with_tokens().filter_map(|e| e.into_token()).map(|t| t.kind()).find(|k| !k.is_trivia())
}

fn arithmetic(op: SyntaxKind) -> bool {
    matches!(op, PLUS | MINUS | STAR | SLASH | SLASH2 | CARET | SHL | SHR | AMP | PIPE | TILDE | DOT2)
}

/// A number, string, boolean or table constructor.
fn literal(n: &SyntaxNode) -> bool {
    match n.kind() {
        TABLE_EXPR => true,
        LITERAL_EXPR => n.first_token().is_some_and(|t| t.kind() != NIL_KW),
        _ => false,
    }
}

/// ambiguity-1: `a + b or 0` and `a or 0 + 1`, where the precedence is likely
/// not what was meant. Unlike LuaLS, `a or "text" .. b` is left alone: there the
/// grouping `a or ("text" .. b)` is nearly always the intent (fallback messages).
fn ambiguity(l: &mut Linter, n: &SyntaxNode) {
    if binary_op(n) != Some(OR_KW) {
        return;
    }
    let mut kids = n.children();
    let (Some(first), Some(second)) = (kids.next(), kids.next()) else { return };
    let has_op = |e: &SyntaxNode| matches!(e.kind(), BINARY_EXPR | UNARY_EXPR);
    let compute = if binary_op(&first).is_some_and(arithmetic) && !has_op(&second) && literal(&second) && !first.children().nth(1).is_some_and(|r| literal(&r)) {
        Some(first)
    } else if binary_op(&second).is_some_and(|op| arithmetic(op) && op != DOT2) && !has_op(&first) && second.children().next().is_some_and(|r| literal(&r)) {
        Some(second)
    } else {
        None
    };
    if let (Some(c), Some(r)) = (compute, trimmed(n)) {
        let msg = format!("Compute `{}` first. You may need to add brackets.", l.src(c.text_range()).trim());
        l.push("ambiguity-1", r, msg);
    }
}

/// unused-function: a function stored in a local that nothing calls, or that only
/// functions which are themselves unused call (LuaLS's colouring: a reference
/// from the file's top level or from any function not stored in a local keeps it).
pub(super) fn unused_functions(l: &mut Linter) {
    let n = l.names;
    let root = l.root.clone();
    // (reported range, body range, local)
    let mut funcs: Vec<(TextRange, TextRange, usize)> = Vec::new();
    for (d, decl) in n.decls.iter().enumerate() {
        if decl.attrib == Some(Attrib::Close) {
            continue;
        }
        let func = match decl.kind {
            DeclKind::LocalFunction => root
                .covering_element(decl.owner)
                .into_node()
                .and_then(|s| s.ancestors().find(|a| a.kind() == LOCAL_FUNCTION_STAT))
                .and_then(|s| {
                    let kw = s.children_with_tokens().find(|e| e.kind() == FUNCTION_KW)?;
                    let body = s.children().find(|c| c.kind() == FUNC_BODY)?;
                    Some((TextRange::new(kw.text_range().start(), body.text_range().end()), body.text_range()))
                }),
            DeclKind::Local => decl.function.and_then(|r| {
                let f = root.covering_element(r).into_node()?.ancestors().find(|a| a.kind() == FUNCTION_EXPR)?;
                let body = f.children().find(|c| c.kind() == FUNC_BODY)?;
                Some((f.text_range(), body.text_range()))
            }),
            _ => None,
        };
        if let Some((r, body)) = func {
            funcs.push((r, body, d));
        }
    }
    // `x = function () end` for a local `x`.
    for stat in root.descendants().filter(|s| s.kind() == ASSIGN_STAT) {
        let vars: Vec<SyntaxNode> = stat.children().find(|c| c.kind() == VAR_LIST).map(|v| v.children().collect()).unwrap_or_default();
        let values: Vec<SyntaxNode> = stat.children().find(|c| c.kind() == EXPR_LIST).map(|v| v.children().collect()).unwrap_or_default();
        for (var, value) in vars.iter().zip(&values) {
            if var.kind() != NAME_EXPR || value.kind() != FUNCTION_EXPR {
                continue;
            }
            let Some(tok) = var.first_token() else { continue };
            let Ok(i) = n.refs.binary_search_by_key(&tok.text_range().start(), |r| r.range.start()) else { continue };
            if let Target::Decl(d) = n.refs[i].target
                && n.decls[d].kind == DeclKind::Local
                && n.decls[d].attrib != Some(Attrib::Close)
                && let Some(body) = value.children().find(|c| c.kind() == FUNC_BODY)
            {
                funcs.push((value.text_range(), body.text_range(), d));
            }
        }
    }
    if funcs.is_empty() {
        return;
    }
    let by_body: HashMap<TextRange, usize> = funcs.iter().enumerate().map(|(i, f)| (f.1, i)).collect();
    let mut by_decl: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, f) in funcs.iter().enumerate() {
        by_decl.entry(f.2).or_default().push(i);
    }
    let mut roots = vec![false; funcs.len()];
    let mut links: Vec<Vec<usize>> = vec![Vec::new(); funcs.len()];
    for r in &n.refs {
        let Target::Decl(d) = r.target else { continue };
        if r.access == Access::Write {
            continue;
        }
        let Some(targets) = by_decl.get(&d) else { continue };
        // The innermost function around the reference.
        let around = root
            .token_at_offset(r.range.start())
            .right_biased()
            .and_then(|t| t.parent_ancestors().find(|a| a.kind() == FUNC_BODY))
            .and_then(|b| by_body.get(&b.text_range()).copied());
        for &t in targets {
            match around {
                Some(f) => links[f].push(t),
                None => roots[t] = true,
            }
        }
    }
    let mut used = vec![false; funcs.len()];
    let mut stack: Vec<usize> = (0..funcs.len()).filter(|&i| roots[i]).collect();
    while let Some(i) = stack.pop() {
        if std::mem::replace(&mut used[i], true) {
            continue;
        }
        stack.extend(links[i].iter().copied());
    }
    for (i, f) in funcs.iter().enumerate() {
        if !used[i] {
            l.push_full("unused-function", f.0, "Unused functions.".into(), Some(Tag::Unnecessary), None);
        }
    }
}
