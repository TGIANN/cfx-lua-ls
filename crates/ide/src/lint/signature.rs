//! LuaLS's documentation-coverage rules, off by default as in LuaLS:
//! `incomplete-signature-doc`, `missing-global-doc`, `missing-local-export-doc`,
//! `global-element` and `no-unknown`.

use super::Linter;
use crate::luacats::Tag;
use crate::names::{Access, DeclKind, Target};
use crate::{TextRange, TextSize};
use cfx_lua_syntax::SyntaxKind::*;
use cfx_lua_syntax::{SyntaxNode, SyntaxToken};

const CODES: &[&str] = &["incomplete-signature-doc", "missing-global-doc", "missing-local-export-doc", "global-element", "no-unknown"];

pub(super) fn run(l: &mut Linter) {
    if !CODES.iter().any(|c| l.enabled(c)) {
        return;
    }
    global_element(l);
    no_unknown(l);
    let root = l.root.clone();
    for body in root.descendants().filter(|b| b.kind() == FUNC_BODY) {
        let Some(holder) = body.parent() else { continue };
        let docs = docs_of(l, &holder);
        let Some(tags) = &docs else { continue };
        if tags.iter().any(|t| matches!(t, Tag::Param { .. } | Tag::Return { .. })) {
            check(l, &body, &holder, Some(tags), None);
        }
    }
    for (holder, name) in global_functions(l) {
        let Some(body) = holder.children().find(|c| c.kind() == FUNC_BODY) else { continue };
        let docs = docs_of(l, &holder);
        check(l, &body, &holder, docs.as_deref(), Some(("missing-global-doc", "global function", name)));
    }
    for (holder, name) in exported_locals(l) {
        let Some(body) = holder.children().find(|c| c.kind() == FUNC_BODY) else { continue };
        let docs = docs_of(l, &holder);
        check(l, &body, &holder, docs.as_deref(), Some(("missing-local-export-doc", "exported local function", name)));
    }
}

/// The doc comment tags (`---` lines) of a function: above its statement, or
/// above `local f = function` / `F = function`; None when it has no `---` line.
fn docs_of(l: &Linter, holder: &SyntaxNode) -> Option<Vec<Tag>> {
    let stat = match holder.kind() {
        FUNCTION_STAT | LOCAL_FUNCTION_STAT => holder.clone(),
        FUNCTION_EXPR => {
            let list = holder.parent().filter(|p| p.kind() == EXPR_LIST)?;
            let stat = list.parent().filter(|p| matches!(p.kind(), LOCAL_STAT | ASSIGN_STAT))?;
            if list.first_child().as_ref() != Some(holder) {
                return None;
            }
            stat
        }
        _ => return None,
    };
    let mut lines = Vec::new();
    for e in stat.children_with_tokens() {
        match e {
            rowan::NodeOrToken::Token(t) if t.kind() == COMMENT => {
                if let Some(rest) = l.text[t.text_range()].strip_prefix("--").filter(|r| r.starts_with('-')) {
                    lines.push((t.text_range().start() + TextSize::from(2), rest));
                }
            }
            rowan::NodeOrToken::Token(t) if t.kind() == WHITESPACE => {}
            _ => break,
        }
    }
    if lines.is_empty() {
        return None;
    }
    Some(crate::luacats::parse_lines(lines).0.into_iter().map(|t| t.tag).collect())
}

/// Reports the parameters without `---@param` and the returned values past
/// the `---@return` count; with `missing`, also a function with neither
/// parameters, returns nor docs. `missing`: (code, kind of function, name).
fn check(l: &mut Linter, body: &SyntaxNode, holder: &SyntaxNode, docs: Option<&[Tag]>, missing: Option<(&'static str, &str, String)>) {
    let params: Vec<SyntaxToken> = body
        .children()
        .find(|c| c.kind() == PARAM_LIST)
        .map(|p| p.children().filter(|c| c.kind() == PARAM).filter_map(|c| c.first_token()).collect())
        .unwrap_or_default();
    let returns: Vec<SyntaxNode> =
        body.descendants().filter(|r| r.kind() == RETURN_STAT && r.ancestors().find(|a| a.kind() == FUNC_BODY).as_ref() == Some(body)).collect();
    let documented = |p: &str| docs.is_some_and(|d| d.iter().any(|t| matches!(t, Tag::Param { name, .. } if name == p)));
    let return_count: usize = docs.map_or(0, |d| d.iter().map(|t| if let Tag::Return { rets, .. } = t { rets.len() } else { 0 }).sum());
    if let Some((code, kind, name)) = &missing
        && params.is_empty()
        && returns.is_empty()
        && docs.is_none()
    {
        // Without the comments before it; a local function's range starts at
        // `function`, as in LuaLS.
        let whole = crate::util::trimmed(holder).unwrap_or(holder.text_range());
        let start = holder
            .children_with_tokens()
            .find(|e| e.kind() == FUNCTION_KW)
            .filter(|_| holder.kind() == LOCAL_FUNCTION_STAT)
            .map_or(whole.start(), |t| t.text_range().start());
        l.push(code, TextRange::new(start, whole.end()), format!("Missing comment for {kind} `{name}`."));
    }
    for p in &params {
        let pname = p.text();
        if pname == "self" || pname == "_" || documented(pname) {
            continue;
        }
        match &missing {
            Some((code, kind, name)) => l.push(code, p.text_range(), format!("Missing @param annotation for parameter `{pname}` in {kind} `{name}`.")),
            None => l.push("incomplete-signature-doc", p.text_range(), format!("Incomplete signature. Missing @param annotation for parameter `{pname}`.")),
        }
    }
    for r in &returns {
        let exprs: Vec<SyntaxNode> = r.children().find(|c| c.kind() == EXPR_LIST).map(|e| e.children().collect()).unwrap_or_default();
        for (i, e) in exprs.iter().enumerate() {
            let index = i + 1;
            if index <= return_count {
                continue;
            }
            match &missing {
                Some((code, kind, name)) => l.push(code, e.text_range(), format!("Missing @return annotation at index `{index}` in {kind} `{name}`.")),
                None => l.push("incomplete-signature-doc", e.text_range(), format!("Incomplete signature. Missing @return annotation at index `{index}`.")),
            }
        }
    }
}

/// `function F()` and `F = function` on a global name.
fn global_functions(l: &Linter) -> Vec<(SyntaxNode, String)> {
    let mut out = Vec::new();
    if !l.enabled("missing-global-doc") {
        return out;
    }
    for n in l.root.descendants() {
        match n.kind() {
            FUNCTION_STAT => {
                let Some(fname) = n.children().find(|c| c.kind() == FUNC_NAME) else { continue };
                let parts: Vec<SyntaxToken> = fname.children_with_tokens().filter_map(|e| e.into_token()).filter(|t| t.kind() == NAME).collect();
                if parts.len() == 1 && l.names.refs.iter().any(|r| r.range == parts[0].text_range() && r.target == Target::Global) {
                    out.push((n.clone(), parts[0].text().to_string()));
                }
            }
            ASSIGN_STAT => {
                let vars: Vec<SyntaxNode> = n.children().find(|c| c.kind() == VAR_LIST).map(|v| v.children().collect()).unwrap_or_default();
                let values: Vec<SyntaxNode> = n.children().find(|c| c.kind() == EXPR_LIST).map(|v| v.children().collect()).unwrap_or_default();
                for (var, value) in vars.iter().zip(values.iter()) {
                    if var.kind() == NAME_EXPR && value.kind() == FUNCTION_EXPR && l.is_global_name(var) {
                        out.push((value.clone(), l.src(var.text_range()).to_string()));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Local functions stored on a local table the file returns (`local M = {}`,
/// `M.f = f`, `return M`).
fn exported_locals(l: &Linter) -> Vec<(SyntaxNode, String)> {
    let mut out = Vec::new();
    if !l.enabled("missing-local-export-doc") {
        return out;
    }
    let n = l.names;
    let decl_of = |e: &SyntaxNode| -> Option<usize> {
        let tok = e.first_token().filter(|_| e.kind() == NAME_EXPR)?;
        match n.refs.iter().find(|r| r.range == tok.text_range())?.target {
            Target::Decl(d) => Some(d),
            Target::Global => None,
        }
    };
    let tables: Vec<usize> = l
        .root
        .descendants()
        .filter(|r| r.kind() == RETURN_STAT)
        .filter_map(|r| r.children().find(|c| c.kind() == EXPR_LIST))
        .flat_map(|e| e.children().collect::<Vec<_>>())
        .filter_map(|e| decl_of(&e))
        .filter(|&d| n.decls[d].kind == DeclKind::Local && n.decls[d].value == crate::names::Value::Table)
        .collect();
    if tables.is_empty() {
        return out;
    }
    for st in l.root.descendants().filter(|s| s.kind() == ASSIGN_STAT) {
        let vars: Vec<SyntaxNode> = st.children().find(|c| c.kind() == VAR_LIST).map(|v| v.children().collect()).unwrap_or_default();
        let values: Vec<SyntaxNode> = st.children().find(|c| c.kind() == EXPR_LIST).map(|v| v.children().collect()).unwrap_or_default();
        for (var, value) in vars.iter().zip(values.iter()) {
            if var.kind() != INDEX_EXPR || !var.first_child().and_then(|o| decl_of(&o)).is_some_and(|d| tables.contains(&d)) {
                continue;
            }
            let Some(d) = decl_of(value) else { continue };
            let decl = &n.decls[d];
            let Some(tok) = l.root.token_at_offset(decl.range.start()).right_biased() else { continue };
            let holder = match decl.kind {
                DeclKind::LocalFunction => tok.parent_ancestors().find(|a| a.kind() == LOCAL_FUNCTION_STAT),
                DeclKind::Local => decl.function.and_then(|f| l.root.covering_element(f).ancestors().find(|a| a.kind() == FUNCTION_EXPR)),
                _ => None,
            };
            if let Some(h) = holder
                && !out.iter().any(|(o, _)| *o == h)
            {
                out.push((h, decl.name.clone()));
            }
        }
    }
    out
}

/// global-element: a global written (`G = 1`, `function G()`) that is not in
/// `diagnostics.globals`, the library or under a `---@class`.
fn global_element(l: &mut Linter) {
    if !l.enabled("global-element") {
        return;
    }
    let lib = crate::library::library();
    let found: Vec<TextRange> = l
        .names
        .refs
        .iter()
        .filter(|r| r.target == Target::Global && r.access == Access::Write)
        .filter(|r| {
            let name = l.src(r.range);
            !l.ctx.config.globals.contains(name) && !lib.has_global(name, l.ctx.game) && !class_above(l, r.range.start())
        })
        .map(|r| r.range)
        .collect();
    for r in found {
        l.push("global-element", r, "Element is global.");
    }
}

fn class_above(l: &Linter, at: TextSize) -> bool {
    let before = &l.text[..u32::from(at) as usize];
    let mut lines = before.lines().rev();
    lines.next();
    lines.take_while(|line| line.trim_start().starts_with("--")).any(|line| line.contains("@class"))
}

/// no-unknown: a local (or loop variable) whose type the inference cannot tell.
fn no_unknown(l: &mut Linter) {
    if !l.enabled("no-unknown") {
        return;
    }
    let found: Vec<TextRange> = l
        .names
        .decls
        .iter()
        .enumerate()
        .filter(|(_, d)| matches!(d.kind, DeclKind::Local | DeclKind::ForVar) && d.name != "_")
        .filter(|(i, _)| l.types().unknown_decl(*i))
        .map(|(_, d)| d.range)
        .collect();
    for r in found {
        l.push("no-unknown", r, "Cannot infer type.");
    }
}
