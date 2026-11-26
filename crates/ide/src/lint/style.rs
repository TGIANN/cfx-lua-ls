//! Lua performance, readability and naming rules (`docs/RULES.md`).

use super::{Fix, Linter};
use crate::library::library;
use crate::names::{Target, is_name};
use crate::util::trimmed;
use crate::TextRange;
use cfx_lua_syntax::SyntaxKind::*;
use cfx_lua_syntax::SyntaxNode;

pub(super) fn node(l: &mut Linter, n: &SyntaxNode) {
    match n.kind() {
        ASSIGN_STAT => concat_in_loop(l, n),
        CALL_EXPR => {
            table_insert(l, n);
            runtime_load(l, n);
        }
        TABLE_EXPR => {
            sequential_keys(l, n);
            constant_table(l, n);
        }
        IF_STAT => boolean_return(l, n),
        INDEX_EXPR => {
            string_key(l, n);
            global_in_loop(l, n);
        }
        FUNCTION_EXPR => closure_in_loop(l, n),
        LOCAL_STAT | LOCAL_FUNCTION_STAT => naming(l, n),
        _ => {}
    }
}

fn concat_in_loop(l: &mut Linter, stat: &SyntaxNode) {
    if !l.in_loop() {
        return;
    }
    let Some(var) = stat.children().find(|c| c.kind() == VAR_LIST).filter(|v| v.children().count() == 1).and_then(|v| v.first_child())
    else {
        return;
    };
    let Some(value) = stat.children().find(|c| c.kind() == EXPR_LIST).filter(|v| v.children().count() == 1).and_then(|v| v.first_child())
    else {
        return;
    };
    let is_concat = value.kind() == BINARY_EXPR && value.children_with_tokens().any(|e| e.kind() == DOT2);
    let same = value.first_child().is_some_and(|f| f.kind() == var.kind() && l.src(f.text_range()) == l.src(var.text_range()));
    // A local declared in the loop starts over on every iteration.
    let fresh = || {
        let Some(lp) = enclosing_loop(stat) else { return false };
        let at = var.text_range();
        l.names.refs.iter().find(|r| at.contains_range(r.range)).is_some_and(|r| match r.target {
            Target::Decl(d) => lp.text_range().contains_range(l.names.decls[d].range),
            Target::Global => false,
        })
    };
    if var.kind() == NAME_EXPR && is_concat && same && !fresh() {
        l.push(
            "perf/concat-in-loop",
            stat.text_range(),
            "`s = s .. x` in a loop copies the whole string on every iteration: collect the parts in a table and join them with `table.concat`.",
        );
    }
}

fn table_insert(l: &mut Linter, call: &SyntaxNode) {
    if l.callee(call).as_deref() != Some("table.insert") {
        return;
    }
    let args = Linter::args(call);
    if args.len() != 2 || args[0].kind() != NAME_EXPR || !call.parent().is_some_and(|p| p.kind() == CALL_STAT) {
        return;
    }
    let t = l.src(args[0].text_range()).to_string();
    let Some(v) = trimmed(&args[1]).map(|r| l.src(r).to_string()) else { return };
    let r = call.text_range();
    let fix = Fix { title: format!("Use {t}[#{t} + 1] = ..."), edits: vec![(r, format!("{t}[#{t} + 1] = {v}"))] };
    l.push_fix("perf/table-insert-append", r, format!("`{t}[#{t} + 1] = v` appends without a function call."), fix);
}

fn runtime_load(l: &mut Linter, call: &SyntaxNode) {
    let Some(p) = l.callee(call) else { return };
    if !matches!(p.as_str(), "load" | "loadstring") {
        return;
    }
    let first = Linter::args(call).into_iter().next();
    let literal = first.as_ref().is_some_and(|a| a.kind() == LITERAL_EXPR);
    if first.is_some() && !literal {
        l.push(
            "perf/runtime-load",
            call.text_range(),
            format!("`{p}` compiles code at run time (about 10 times slower than calling a function); prefer a function or a table of functions."),
        );
    }
}

fn integer_key(l: &Linter, field: &SyntaxNode) -> Option<i64> {
    let key = field.children().next().filter(|k| k.kind() == LITERAL_EXPR)?;
    l.src(key.text_range()).trim().parse().ok()
}

fn sequential_keys(l: &mut Linter, table: &SyntaxNode) {
    let fields: Vec<SyntaxNode> = table.children().collect();
    if fields.len() < 2 || fields.iter().any(|f| f.kind() != FIELD_INDEXED) {
        return;
    }
    if !fields.iter().enumerate().all(|(i, f)| integer_key(l, f) == Some(i as i64 + 1)) {
        return;
    }
    // Delete each `[i] = `.
    let mut edits = Vec::new();
    for f in &fields {
        let (Some(open), Some(value)) = (f.first_token(), f.children().nth(1)) else { return };
        let Some(v) = trimmed(&value) else { return };
        edits.push((TextRange::new(open.text_range().start(), v.start()), String::new()));
    }
    let fix = Fix { title: "Remove the explicit keys".into(), edits };
    l.push_fix(
        "perf/sequential-index-keys",
        trimmed(table).unwrap_or(table.text_range()),
        "Explicit `[1] =`, `[2] =` keys put the values in the hash part: write `{ a, b, ... }`.",
        fix,
    );
}

fn is_constant(expr: &SyntaxNode) -> bool {
    match expr.kind() {
        LITERAL_EXPR => true,
        UNARY_EXPR => expr.first_child().is_some_and(|e| e.kind() == LITERAL_EXPR),
        TABLE_EXPR => expr.children().all(|f| {
            let parts: Vec<SyntaxNode> = f.children().collect();
            match f.kind() {
                FIELD_POS | FIELD_NAMED => parts.last().is_some_and(is_constant),
                FIELD_INDEXED => parts.iter().all(is_constant),
                _ => false,
            }
        }),
        _ => false,
    }
}

/// A constant table built on every loop iteration only to be passed to a library
/// function (which does not keep it).
fn constant_table(l: &mut Linter, table: &SyntaxNode) {
    if !l.in_loop() || table.children().next().is_none() || !is_constant(table) {
        return;
    }
    let Some(call) = table.parent().filter(|a| a.kind() == ARG_LIST).and_then(|a| a.parent()) else { return };
    if call.parent().is_some_and(|p| p.kind() == TABLE_EXPR) {
        return;
    }
    let Some(p) = l.callee(&call) else { return };
    if library().get(&p).is_empty() {
        return;
    }
    l.push(
        "perf/constant-table-in-loop",
        trimmed(table).unwrap_or(table.text_range()),
        "This constant table is built on every iteration: create it once before the loop.",
    );
}

/// The innermost loop statement around `n`.
fn enclosing_loop(n: &SyntaxNode) -> Option<SyntaxNode> {
    n.ancestors().skip(1).find(|a| matches!(a.kind(), WHILE_STAT | REPEAT_STAT | NUMERIC_FOR_STAT | GENERIC_FOR_STAT))
}

/// A function expression passed to a Lua library function on every iteration
/// of a loop that never waits, using no local of the loop (loop variables, locals of its
/// body): the same closure built each time.
fn closure_in_loop(l: &mut Linter, f: &SyntaxNode) {
    if !l.in_loop() {
        return;
    }
    // Only one passed straight to a call (`table.sort(t, function ...)`), in a
    // loop that never waits: a stored closure or a paced loop is not the cost.
    // Only to a Lua library function (`table.sort`, `string.gsub`, `pcall`):
    // event handlers and callbacks of other APIs are registrations.
    let Some(call) = f.parent().filter(|p| p.kind() == ARG_LIST).and_then(|a| a.parent()) else { return };
    let Some(callee) = l.callee(&call) else { return };
    if !library().get(&callee).iter().any(|i| i.origin == crate::library::Origin::Lua) {
        return;
    }
    let Some(lp) = enclosing_loop(f) else { return };
    if yields(l, &lp) {
        return;
    }
    let (loop_range, fn_range) = (lp.text_range(), f.text_range());
    let captures = l.names.refs.iter().filter(|r| fn_range.contains_range(r.range)).any(|r| match r.target {
        Target::Decl(d) => {
            let decl = l.names.decls[d].range;
            loop_range.contains_range(decl) && !fn_range.contains_range(decl)
        }
        Target::Global => false,
    });
    if captures {
        return;
    }
    let Some(head) = f.first_token() else { return };
    l.push(
        "perf/closure-in-loop",
        head.text_range(),
        "This function is created again on every iteration and uses nothing of the loop: create it once before the loop.",
    );
}

/// The loop waits (`Wait`, `coroutine.yield`): it is paced, not hot.
fn yields(l: &Linter, lp: &SyntaxNode) -> bool {
    lp.descendants().filter(|c| c.kind() == CALL_EXPR).any(|c| {
        c.first_child().is_some_and(|f| matches!(l.src(f.text_range()), "Wait" | "Citizen.Wait" | "coroutine.yield" | "Citizen.Await"))
    })
}

/// The libraries whose functions a hot loop reads through two lookups.
const HOT_LIBS: &[&str] = &["math", "string", "table", "utf8", "bit32"];

/// `math.floor(x)` called in a `for` loop that never waits: the global and the
/// field are looked up on every iteration. Once per name and loop.
fn global_in_loop(l: &mut Linter, e: &SyntaxNode) {
    if !matches!(l.frames.last(), Some(super::Frame::Loop { finite: true })) {
        return;
    }
    let Some(object) = e.first_child().filter(|o| o.kind() == NAME_EXPR) else { return };
    let lib = l.src(object.text_range());
    if !HOT_LIBS.contains(&lib) || !l.is_global_name(&object) {
        return;
    }
    if !e.parent().is_some_and(|p| p.kind() == CALL_EXPR && p.first_child().as_ref() == Some(e)) {
        return;
    }
    let Some(lp) = enclosing_loop(e) else { return };
    if yields(l, &lp) {
        return;
    }
    let name = l.src(e.text_range()).to_string();
    let first = lp
        .descendants()
        .filter(|c| c.kind() == INDEX_EXPR && enclosing_loop(c).as_ref() == Some(&lp))
        .find(|c| l.src(c.text_range()) == name);
    if first.as_ref() != Some(e) {
        return;
    }
    let local = name.rsplit('.').next().unwrap_or(&name).to_string();
    l.push(
        "perf/global-in-loop",
        e.text_range(),
        format!("`{name}` is looked up on every iteration of this loop: `local {local} = {name}` before it."),
    );
}

/// `if c then return true else return false end`.
fn boolean_return(l: &mut Linter, stat: &SyntaxNode) {
    let clauses: Vec<SyntaxNode> = stat.children().filter(|c| matches!(c.kind(), ELSEIF_CLAUSE | ELSE_CLAUSE)).collect();
    if clauses.len() != 1 || clauses[0].kind() != ELSE_CLAUSE {
        return;
    }
    let returned = |block: Option<SyntaxNode>| -> Option<String> {
        let block = block?;
        let mut stats = block.children();
        let ret = stats.next().filter(|s| s.kind() == RETURN_STAT)?;
        if stats.next().is_some() {
            return None;
        }
        let list = ret.children().find(|c| c.kind() == EXPR_LIST)?;
        (list.children().count() == 1).then(|| l.src(list.text_range()).trim().to_string())
    };
    let then = returned(stat.children().find(|c| c.kind() == BLOCK));
    let other = returned(clauses[0].children().find(|c| c.kind() == BLOCK));
    let (Some(then), Some(other)) = (then, other) else { return };
    let negate = match (then.as_str(), other.as_str()) {
        ("true", "false") => false,
        ("false", "true") => true,
        _ => return,
    };
    let Some(cond) = stat.children().find(|c| !matches!(c.kind(), BLOCK | ELSEIF_CLAUSE | ELSE_CLAUSE)) else { return };
    let Some(r) = trimmed(stat) else { return };
    let c = l.src(trimmed(&cond).unwrap_or(cond.text_range()));
    let boolean = match cond.kind() {
        BINARY_EXPR => cond
            .children_with_tokens()
            .any(|e| matches!(e.kind(), EQ2 | NEQ | LT | GT | LTEQ | GTEQ)),
        UNARY_EXPR => cond.first_token().is_some_and(|t| t.kind() == NOT_KW),
        _ => false,
    };
    let replacement = if negate { format!("return not ({c})") } else { format!("return {c}") };
    let msg = format!("`{replacement}` says the same.");
    if boolean {
        let fix = Fix { title: format!("Use {replacement}"), edits: vec![(r, replacement)] };
        l.push_fix("style/redundant-boolean-return", r, msg, fix);
    } else {
        // `c` may not be a boolean: `return c` would change the returned value.
        let msg = format!("`return {}{c}{}` returns the condition as a boolean.", if negate { "not " } else { "not not (" }, if negate { "" } else { ")" });
        l.push("style/redundant-boolean-return", r, msg);
    }
}

fn string_key(l: &mut Linter, expr: &SyntaxNode) {
    let mut tokens = expr.children_with_tokens().filter_map(|e| e.into_token());
    let Some(open) = tokens.find(|t| t.kind() == L_BRACK) else { return };
    let Some(close) = expr.children_with_tokens().filter_map(|e| e.into_token()).find(|t| t.kind() == R_BRACK) else { return };
    let Some(key) = expr.children().nth(1).filter(|k| k.kind() == LITERAL_EXPR) else { return };
    let s = l.src(key.text_range()).trim();
    let Some(name) = crate::util::unquote(s) else { return };
    // `exports["name"]` is the usual way to name a resource, many of which are not
    // identifiers.
    let object = expr.first_child().map(|o| l.src(o.text_range()).trim().to_string());
    if !is_name(name) || object.as_deref() == Some("exports") {
        return;
    }
    let r = TextRange::new(open.text_range().start(), close.text_range().end());
    let fix = Fix { title: format!("Use .{name}"), edits: vec![(r, format!(".{name}"))] };
    l.push_fix("style/string-key-access", r, format!("`.{name}` reads the same field."), fix);
}

fn is_camel(name: &str) -> bool {
    let core = name.trim_start_matches('_');
    !core.contains('_') || core.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// `my_value` -> `myValue`, `Bad_Name` -> `badName`; an all-caps first part stays
/// (`ESX_data` -> `ESXData`).
fn to_camel(name: &str) -> String {
    let lead = name.len() - name.trim_start_matches('_').len();
    let mut out = name[..lead].to_string();
    let mut upper = false;
    let core = &name[lead..];
    let mut chars = core.chars();
    let capitalized = matches!((chars.next(), chars.next()), (Some(a), Some(b)) if a.is_ascii_uppercase() && b.is_ascii_lowercase());
    let core = if capitalized { format!("{}{}", core[..1].to_ascii_lowercase(), &core[1..]) } else { core.to_string() };
    for c in core.chars() {
        if c == '_' {
            upper = !out[lead..].is_empty();
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

fn is_upper_snake(name: &str) -> bool {
    name.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_') && name.chars().any(|c| c.is_ascii_uppercase())
}

fn naming(l: &mut Linter, stat: &SyntaxNode) {
    let names: Vec<(cfx_lua_syntax::SyntaxToken, Option<SyntaxNode>, bool)> = match stat.kind() {
        LOCAL_FUNCTION_STAT => stat
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| t.kind() == NAME)
            .map(|t| (t, None, false))
            .into_iter()
            .collect(),
        _ => {
            let values: Vec<SyntaxNode> =
                stat.children().find(|c| c.kind() == EXPR_LIST).map(|v| v.children().collect()).unwrap_or_default();
            stat.children()
                .filter(|c| c.kind() == LOCAL_NAME)
                .enumerate()
                .filter_map(|(i, ln)| {
                    let name = ln.children_with_tokens().filter_map(|e| e.into_token()).find(|t| t.kind() == NAME)?;
                    let constant = ln.children().any(|a| a.kind() == ATTRIB && l.src(a.text_range()).contains("const"));
                    Some((name, values.get(i).cloned(), constant))
                })
                .collect()
        }
    };
    for (token, value, constant) in names {
        let name = l.src(token.text_range());
        if constant {
            // Only literal constants: `local GetEntityCoords <const> = GetEntityCoords` is a cached function.
            let literal = value.as_ref().is_some_and(|v| v.kind() == LITERAL_EXPR && v.first_token().is_some_and(|t| matches!(t.kind(), NUMBER | STRING)));
            if literal && !is_upper_snake(name) {
                l.push("naming/const", token.text_range(), format!("Constant `{name}` is not in UPPER_SNAKE_CASE."));
            }
            continue;
        }
        if is_camel(name) {
            continue;
        }
        let better = to_camel(name);
        let fix = rename_fix(l, token.text_range(), &better);
        let msg = format!("Local `{name}` is not in camelCase: `{better}`.");
        match fix {
            Some(fix) => l.push_fix("naming/local", token.text_range(), msg, fix),
            None => l.push("naming/local", token.text_range(), msg),
        }
    }
}

/// Renames a local everywhere in the file, unless the new name is already used.
fn rename_fix(l: &Linter, decl: TextRange, new: &str) -> Option<Fix> {
    let n = l.names;
    if n.decls.iter().any(|d| d.name == new) || n.refs.iter().any(|r| l.src(r.range) == new) {
        return None;
    }
    let d = n.decls.iter().position(|d| d.range == decl)?;
    let mut edits = vec![(decl, new.to_string())];
    edits.extend(n.refs.iter().filter(|r| r.target == Target::Decl(d)).map(|r| (r.range, new.to_string())));
    Some(Fix { title: format!("Rename to {new}"), edits })
}
