//! LuaLS's rules (`script/core/diagnostics`), same messages and ranges.

use super::{Linter, Severity, Tag};
use crate::library::library;
use crate::names::{Access, DeclKind, Target, Value};
use crate::util::trimmed;
use crate::{TextRange, TextSize};
use cfx_lua_syntax::SyntaxKind::*;
use cfx_lua_syntax::SyntaxNode;
use std::collections::{HashMap, HashSet};

/// The rules that only need the name resolution.
pub(super) fn names(l: &mut Linter) {
    let n = l.names;
    // unused-local
    let mut reads = vec![(false, false); n.decls.len()]; // (strong, weak)
    for r in &n.refs {
        if let Target::Decl(d) = r.target {
            match r.access {
                Access::Read | Access::ReadWrite => reads[d].0 = true,
                Access::FieldSet => reads[d].1 = true,
                Access::Write => {}
            }
        }
    }
    for (d, decl) in n.decls.iter().enumerate() {
        match decl.kind {
            DeclKind::Local | DeclKind::LocalFunction | DeclKind::Param | DeclKind::ForVar => {}
            DeclKind::Label => {
                if !reads[d].0 {
                    let msg = format!("Unused label `{}`.", decl.name);
                    l.push_full("unused-label", decl.range, msg, Some(Tag::Unnecessary), None);
                }
                continue;
            }
            DeclKind::SelfParam => continue,
        }
        if decl.name == "_" || decl.name == "_ENV" || decl.attrib == Some(crate::names::Attrib::Close) {
            continue;
        }
        // `diagnostics.unusedLocalExclude`: `ll_*`.
        if l.ctx.config.unused_local_exclude.iter().any(|g| crate::manifest::glob_exact(g, &decl.name)) {
            continue;
        }
        let (strong, weak) = reads[d];
        if strong || (weak && decl.value != Value::Table) || has_class_doc(l, decl.owner) {
            continue;
        }
        let msg = format!("Unused local `{}`.", decl.name);
        l.push_full("unused-local", decl.range, msg.clone(), Some(Tag::Unnecessary), None);
        for r in n.refs.iter().filter(|r| r.target == Target::Decl(d)) {
            l.push_full("unused-local", r.range, msg.clone(), Some(Tag::Unnecessary), None);
        }
    }
    for v in &n.varargs {
        if !v.used {
            l.push_full("unused-vararg", v.range, "Unused vararg.".into(), Some(Tag::Unnecessary), None);
        }
    }
    for &(d, _) in &n.shadows {
        let decl = &n.decls[d];
        l.push("redefined-local", decl.range, format!("Redefined local `{}`.", decl.name));
    }

    // Globals the file writes count as defined for the whole file.
    let written: HashSet<String> = crate::workspace::global_writes_in(&l.root, l.text, n).into_iter().collect();
    let lib = library();
    let config_globals = &l.ctx.config.globals;
    let mut known: HashMap<&str, bool> = HashMap::new();
    // `_ENV` locals and global `_ENV = v` assignments, found once.
    let (env_locals, env_sets): (Vec<usize>, Vec<SyntaxNode>) = if l.text.contains("_ENV") {
        (
            n.decls.iter().enumerate().filter(|(_, d)| d.name == "_ENV" && matches!(d.kind, DeclKind::Local | DeclKind::Param)).map(|(i, _)| i).collect(),
            l.root
                .descendants()
                .filter(|s| s.kind() == ASSIGN_STAT)
                .filter(|s| {
                    s.children().find(|c| c.kind() == VAR_LIST).and_then(|v| v.first_child()).is_some_and(|t| t.kind() == NAME_EXPR && l.src(t.text_range()) == "_ENV")
                })
                .collect(),
        )
    } else {
        (Vec::new(), Vec::new())
    };
    let envs = !env_locals.is_empty() || !env_sets.is_empty();
    for r in &n.refs {
        if r.target != Target::Global {
            continue;
        }
        let name = l.src(r.range);
        // global-in-nil-env, undefined-env-child: `_ENV` replaced.
        if envs && name != "_ENV" {
            match env_at(l, r.range.start(), &env_locals, &env_sets) {
                Some(Env::Nil) => {
                    l.push("global-in-nil-env", r.range, "Invalid global (`_ENV` is `nil`).");
                    continue;
                }
                Some(Env::Class(class, value)) => {
                    if !env_has(l, &class, value.as_deref(), name) {
                        l.push("undefined-env-child", r.range, format!("Undefined variable `{name}` (overloaded `_ENV` )."));
                    }
                    continue;
                }
                Some(Env::Other) => continue,
                None => {}
            }
        }
        if r.access == Access::Write {
            // lowercase-global
            let first = name.chars().find(|c| c.is_alphanumeric());
            if first.is_some_and(|c| c.is_lowercase())
                && !config_globals.contains(name)
                && !lib.has_global(name, l.ctx.game)
                && !has_class_doc_at(l, r.range.start())
                && !(l.ctx.used_elsewhere)(name)
            {
                l.push("lowercase-global", r.range, "Global variable in lowercase initial, Did you miss `local` or misspell it?");
            }
            continue;
        }
        if l.ctx.globals_unknown {
            continue;
        }
        let defined = *known.entry(name).or_insert_with(|| {
            written.contains(name)
                || matches!(name, "_ENV" | "_G")
                || config_globals.contains(name)
                || lib.has_global(name, l.ctx.game)
                || (l.ctx.ox_lib && matches!(name, "lib" | "cache"))
                || (l.ctx.defined_elsewhere)(name)
        });
        if !defined {
            let mut msg = format!("Undefined global `{name}`.");
            if matches!(name, "lib" | "cache") {
                msg.push_str(" ox_lib is not loaded for this file: add '@ox_lib/init.lua' to shared_scripts in fxmanifest.lua.");
            }
            l.push("undefined-global", r.range, msg);
        }
    }
}

/// What `_ENV` holds where a global name is used.
enum Env {
    /// `local _ENV`, `local _ENV = nil`, `_ENV = nil`.
    Nil,
    /// `---@type C` on it: the class and the global path of the value (`io`).
    Class(String, Option<String>),
    /// Anything else (`{}`, a parameter): any name may be there.
    Other,
}

fn env_at(l: &Linter, at: TextSize, locals: &[usize], sets: &[SyntaxNode]) -> Option<Env> {
    let n = l.names;
    // The innermost visible local `_ENV`, else the last global `_ENV = v` before.
    let local = locals
        .iter()
        .map(|&i| &n.decls[i])
        .filter(|d| d.range.end() <= at)
        // A local is not visible in its own value.
        .filter(|d| d.kind == DeclKind::Param || d.owner.end() <= at)
        .filter(|d| super::luadoc::scope_of(l, d.range).is_some_and(|s| s.contains(at)))
        .max_by_key(|d| d.range.start());
    let stat = match local {
        Some(d) if d.kind == DeclKind::Param => return Some(Env::Other),
        Some(d) => l.root.covering_element(d.range).ancestors().find(|a| a.kind() == LOCAL_STAT)?,
        None => sets.iter().rev().find(|s| s.text_range().end() <= at)?.clone(),
    };
    let value = stat.children().find(|c| c.kind() == EXPR_LIST).and_then(|e| e.first_child());
    if value.as_ref().is_none_or(|v| v.kind() == LITERAL_EXPR && l.src(v.text_range()) == "nil") {
        return Some(Env::Nil);
    }
    let class = stat.children_with_tokens().filter_map(|e| e.into_token()).take_while(|t| matches!(t.kind(), COMMENT | WHITESPACE)).find_map(|t| {
        let rest = t.text().strip_prefix("---@type")?.trim();
        let c: String = rest.chars().take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '.')).collect();
        (!c.is_empty() && c.len() == rest.len()).then_some(c)
    });
    Some(match class {
        Some(c) => Env::Class(c, value.map(|v| l.src(v.text_range()).to_string())),
        None => Env::Other,
    })
}

/// A name the `_ENV` class has: a `---@field` of it (file or library) or a
/// library field of the value (`io.stderr`).
fn env_has(l: &Linter, class: &str, value: Option<&str>, name: &str) -> bool {
    let fields = |c: &crate::api::ClassDef| c.name == class && c.fields.iter().any(|f| f.name == name);
    crate::api::doc_classes(l.text).iter().any(fields)
        || crate::library::classes().get(class).is_some_and(|v| v.iter().any(fields))
        || value.is_some_and(|v| !library().get(&format!("{v}.{name}")).is_empty())
}

/// A `---@class` in the comments of the statement that declares something.
fn has_class_doc(l: &Linter, owner: TextRange) -> bool {
    let head = &l.text[owner];
    let code_start = head.lines().position(|line| !line.trim_start().starts_with("--")).unwrap_or(0);
    head.lines().take(code_start).any(|line| line.contains("@class"))
}

/// A `---@class` on the lines right above `at`.
fn has_class_doc_at(l: &Linter, at: TextSize) -> bool {
    let before = &l.text[..u32::from(at) as usize];
    let mut lines = before.lines().rev();
    lines.next(); // the statement's own line
    lines.take_while(|line| line.trim_start().starts_with("--")).any(|line| line.contains("@class"))
}

/// trailing-space: a line ending in spaces or tabs, outside strings and comments.
pub(super) fn text(l: &mut Linter, root: &SyntaxNode) {
    let text = l.text;
    let mut skip: Vec<TextRange> = root
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| matches!(t.kind(), STRING | LONG_STRING | COMMENT | BLOCK_COMMENT))
        .map(|t| t.text_range())
        .collect();
    skip.sort_by_key(|r| r.start());
    let inside = |o: TextSize| {
        let i = skip.partition_point(|r| r.start() <= o);
        i > 0 && skip[i - 1].contains(o)
    };
    let mut start = 0usize;
    for line in text.split_inclusive('\n') {
        let content = line.trim_end_matches(['\n', '\r']);
        let trimmed_len = content.trim_end_matches([' ', '\t']).len();
        if trimmed_len < content.len() {
            let last = TextSize::from((start + content.len() - 1) as u32);
            if !inside(last) {
                let r = TextRange::new(TextSize::from((start + trimmed_len) as u32), last + TextSize::from(1));
                let msg = if trimmed_len == 0 { "Line with spaces only." } else { "Line with trailing space." };
                l.push("trailing-space", r, msg);
            }
        }
        start += line.len();
    }
}

fn block_is_empty(block: Option<SyntaxNode>) -> bool {
    block.is_none_or(|b| b.children().next().is_none())
}

/// A number literal, or `-` and one (LuaLS's parser folds the sign into the number).
fn number(l: &Linter, expr: &SyntaxNode) -> Option<f64> {
    if expr.kind() == UNARY_EXPR && expr.first_token().is_some_and(|t| t.kind() == MINUS) {
        return number(l, &expr.first_child()?).map(|v| -v);
    }
    if expr.kind() != LITERAL_EXPR {
        return None;
    }
    let t = expr.first_token().filter(|t| t.kind() == NUMBER)?;
    let s = l.src(t.text_range());
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        return i64::from_str_radix(hex, 16).ok().map(|v| v as f64);
    }
    s.parse().ok()
}

pub(super) fn node(l: &mut Linter, n: &SyntaxNode) {
    match n.kind() {
        IF_STAT => {
            let mut blocks = vec![n.children().find(|c| c.kind() == BLOCK)];
            for clause in n.children().filter(|c| matches!(c.kind(), ELSEIF_CLAUSE | ELSE_CLAUSE)) {
                blocks.push(clause.children().find(|c| c.kind() == BLOCK));
            }
            if blocks.into_iter().all(block_is_empty)
                && let Some(r) = trimmed(n)
            {
                l.push_full("empty-block", r, "Empty block.".into(), Some(Tag::Unnecessary), None);
            }
        }
        NUMERIC_FOR_STAT | GENERIC_FOR_STAT => {
            if block_is_empty(n.children().find(|c| c.kind() == BLOCK))
                && let Some(r) = trimmed(n)
            {
                l.push_full("empty-block", r, "Empty block.".into(), Some(Tag::Unnecessary), None);
            }
            if n.kind() == NUMERIC_FOR_STAT {
                count_down(l, n);
            }
        }
        BLOCK => {
            // `break;`: the `;` is no statement to LuaLS.
            let stats: Vec<SyntaxNode> = n.children().filter(|s| s.kind() != EMPTY_STAT).collect();
            if let Some(i) = stats.iter().position(|s| s.kind() == BREAK_STAT)
                && i + 1 < stats.len()
                && let (Some(a), Some(b)) = (trimmed(&stats[i + 1]), trimmed(stats.last().unwrap()))
            {
                let r = TextRange::new(a.start(), b.end());
                l.push_full("code-after-break", r, "Unable to execute code after `break`.".into(), Some(Tag::Unnecessary), None);
            }
        }
        LOCAL_STAT | ASSIGN_STAT => balance(l, n),
        TABLE_EXPR => duplicate_index(l, n),
        CALL_EXPR => newline_call(l, n),
        _ => {}
    }
}

/// unbalanced-assignments and redundant-value.
fn balance(l: &mut Linter, stat: &SyntaxNode) {
    if !stat.children_with_tokens().any(|e| e.kind() == EQ) {
        return;
    }
    let vars: Vec<SyntaxNode> = match stat.kind() {
        LOCAL_STAT => stat.children().filter(|c| c.kind() == LOCAL_NAME).collect(),
        _ => stat.children().find(|c| c.kind() == VAR_LIST).map(|v| v.children().collect()).unwrap_or_default(),
    };
    let values: Vec<SyntaxNode> =
        stat.children().find(|c| c.kind() == EXPR_LIST).map(|v| v.children().collect()).unwrap_or_default();
    if values.is_empty() {
        return;
    }
    if values.len() < vars.len() {
        let last = values.last().unwrap().kind();
        if matches!(last, CALL_EXPR | METHOD_CALL_EXPR | VARARG_EXPR) {
            return;
        }
        for var in &vars[values.len()..] {
            let r = match var.kind() {
                INDEX_EXPR => var.text_range(),
                _ => var.children_with_tokens().find(|e| e.kind() == NAME).map_or(var.text_range(), |e| e.text_range()),
            };
            l.push(
                "unbalanced-assignments",
                r,
                "The value is assigned as `nil` because the number of values is not enough. In Lua, `x, y = 1 ` is equivalent to `x, y = 1, nil` .",
            );
        }
    } else if values.len() > vars.len() {
        let msg = format!("Only has {} variables, but you set {} values.", vars.len(), values.len());
        for v in &values[vars.len()..] {
            if let Some(r) = trimmed(v) {
                l.push_full("redundant-value", r, msg.clone(), Some(Tag::Unnecessary), None);
            }
        }
    }
}

fn duplicate_index(l: &mut Linter, table: &SyntaxNode) {
    let mut keys: HashMap<String, Vec<TextRange>> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut position = 0;
    for field in table.children() {
        let key = match field.kind() {
            FIELD_POS => {
                position += 1;
                trimmed(&field).map(|r| (position.to_string(), r))
            }
            FIELD_NAMED | FIELD_SET => field
                .children_with_tokens()
                .find(|e| e.kind() == NAME)
                .map(|e| (l.src(e.text_range()).to_string(), e.text_range())),
            FIELD_INDEXED => field.children().next().and_then(|k| {
                let t = k.first_token()?;
                let s = l.src(t.text_range());
                let name = match t.kind() {
                    STRING if k.kind() == LITERAL_EXPR && !s.contains('\\') => crate::util::unquote(s)?.to_string(),
                    NUMBER if k.kind() == LITERAL_EXPR => {
                        let v = number(l, &k)?;
                        if v.fract() == 0.0 { format!("{}", v as i64) } else { v.to_string() }
                    }
                    _ => return None,
                };
                Some((name, trimmed(&k)?))
            }),
            _ => None,
        };
        if let Some((k, r)) = key {
            if !keys.contains_key(&k) {
                order.push(k.clone());
            }
            keys.entry(k).or_default().push(r);
        }
    }
    for k in order {
        let defs = &keys[&k];
        if defs.len() < 2 {
            continue;
        }
        let msg = format!("Duplicate index `{k}`.");
        for (i, &r) in defs.iter().enumerate() {
            if i + 1 < defs.len() {
                l.push_level("duplicate-index", r, msg.clone(), Severity::Hint, Some(Tag::Unnecessary));
            } else {
                l.push("duplicate-index", r, msg.clone());
            }
        }
    }
}

fn newline_call(l: &mut Linter, call: &SyntaxNode) {
    let Some(args) = call.children().find(|c| c.kind() == ARG_LIST) else { return };
    let Some(callee) = call.first_child() else { return };
    let used = call.parent().is_some_and(|p| {
        matches!(p.kind(), CALL_EXPR | INDEX_EXPR | METHOD_CALL_EXPR) && p.first_child().as_ref() == Some(call)
    });
    if !used || args.children().count() != 1 {
        return;
    }
    let a = l.src(args.text_range());
    if !a.starts_with('(') || !a.ends_with(')') {
        return;
    }
    let Some(callee_r) = trimmed(&callee) else { return };
    let between = &l.text[u32::from(callee_r.end()) as usize..u32::from(args.text_range().start()) as usize];
    if !between.contains('\n') {
        return;
    }
    let msg = format!(
        "Will be interpreted as `{}{}`. It may be necessary to add a `,`.",
        l.src(callee_r),
        a
    );
    l.push("newline-call", TextRange::new(callee_r.start(), args.text_range().end()), msg);
}

fn count_down(l: &mut Linter, stat: &SyntaxNode) {
    let exprs: Vec<SyntaxNode> = stat.children().filter(|c| c.kind() != BLOCK).collect();
    let (Some(init), Some(max)) = (exprs.first(), exprs.get(1)) else { return };
    let Some(max_n) = number(l, max) else { return };
    let init_n = number(l, init);
    if init_n.is_some_and(|i| i <= max_n) || (init_n.is_none() && max_n != 1.0) {
        return;
    }
    let (Some(ir), Some(mr)) = (trimmed(init), trimmed(max)) else { return };
    let head = &l.text[u32::from(ir.start()) as usize..u32::from(mr.end()) as usize];
    match exprs.get(2) {
        None => l.push("count-down-loop", TextRange::new(ir.start(), mr.end()), format!("Do you mean `{head}, -1` ?")),
        Some(step) => {
            if number(l, step).is_some_and(|s| s > 0.0)
                && let Some(sr) = trimmed(step)
            {
                let msg = format!("Do you mean `{head}, -{}` ?", l.src(sr));
                l.push("count-down-loop", TextRange::new(ir.start(), sr.end()), msg);
            }
        }
    }
}
