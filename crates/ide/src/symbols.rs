//! Document symbols (the outline), ported from LuaLS `script/core/document-symbol.lua`:
//! the same names, details, kinds and ranges, so its fixtures pass unchanged.
//!
//! Every definition becomes a symbol (`local`, assignment to a name or `a.b`,
//! `function`, table field, parameter, loop variable), plus anonymous functions and
//! tables that are not a definition's value, and `if` / `while` / `repeat` / `for`
//! blocks. Symbols are collected flat, sorted by where their value starts, then nested:
//! a symbol is the child of the innermost earlier one whose value holds its name.
//!
//! CfxLua shapes LuaLS has no rule for:
//! - compound assignment (`x += 1`) updates a value and defines nothing: no symbol;
//! - a set constructor field (`{ .name }`, `{ .name = v }`) is a field named `name`;
//! - a hash string (`` `name` ``) is a number, shown as written.
//!
//! LuaLS shows numbers and strings by their value (`0x10` as `16`, `'x'` as `"x"`);
//! so do we, except a string with invalid UTF-8 or a hex float, shown as written.
//! Names and details are cut to `MAX_TEXT` bytes: a minified file would otherwise
//! put whole function bodies into them.

use crate::util::{if_clauses, statement_function, token, tokens, trimmed};
use crate::{Parse, TextRange};
use cfx_lua_syntax::SyntaxKind::{self, *};
use cfx_lua_syntax::{SyntaxNode, SyntaxToken};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Package,
    Method,
    Function,
    Variable,
    Constant,
    String,
    Number,
    Boolean,
    Array,
    Object,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentSymbol {
    /// Empty only for a function with no usable name (`local function` alone).
    pub name: String,
    pub detail: String,
    pub kind: SymbolKind,
    pub range: TextRange,
    pub selection_range: TextRange,
    /// Where children nest: the value, the function, the table or the block.
    pub value_range: Option<TextRange>,
    pub children: Vec<DocumentSymbol>,
}

/// Longest name or detail, in bytes, before it is cut with `…`.
pub const MAX_TEXT: usize = 120;

pub fn document_symbols(parse: &Parse, text: &str) -> Vec<DocumentSymbol> {
    let mut b = Builder { text, out: Vec::new(), used: HashSet::new() };
    for node in parse.syntax().descendants() {
        b.node(&node);
    }
    nest(b.out)
}

/// A definition's value: an expression, or the function of a `function` statement.
enum Value {
    Expr(SyntaxNode),
    Function { range: TextRange, body: Option<SyntaxNode> },
}

struct Builder<'a> {
    text: &'a str,
    out: Vec<DocumentSymbol>,
    /// Functions and tables that are a definition's value: not anonymous.
    used: HashSet<(SyntaxKind, TextRange)>,
}

impl Builder<'_> {
    fn node(&mut self, node: &SyntaxNode) {
        match node.kind() {
            LOCAL_STAT => self.local_stat(node),
            LOCAL_FUNCTION_STAT => self.local_function(node),
            FUNCTION_STAT => self.function_stat(node),
            ASSIGN_STAT => self.assign_stat(node),
            PARAM => {
                if let Some(name) = token(node, NAME)
                    && token(node, DOT3).is_none()
                {
                    let r = name.text_range();
                    self.value(name.text().into(), r, r, SymbolKind::Constant, false, None);
                }
            }
            TABLE_EXPR => {
                self.anonymous(node);
                self.fields(node);
            }
            FUNCTION_EXPR => self.anonymous(node),
            IF_STAT => self.if_stat(node),
            WHILE_STAT => self.block("while", node, WHILE_KW, DO_KW),
            NUMERIC_FOR_STAT | GENERIC_FOR_STAT => {
                self.block("for", node, FOR_KW, DO_KW);
                for name in tokens(node).filter(|t| t.kind() == NAME) {
                    let r = name.text_range();
                    self.value(name.text().into(), r, r, SymbolKind::Variable, false, None);
                }
            }
            REPEAT_STAT => self.repeat_stat(node),
            _ => {}
        }
    }

    fn local_stat(&mut self, node: &SyntaxNode) {
        let exprs = exprs(node);
        for (i, local) in node.children().filter(|c| c.kind() == LOCAL_NAME).enumerate() {
            let Some(name) = token(&local, NAME) else { continue };
            let r = name.text_range();
            let value = value_at(&exprs, i).map(|e| self.value_of(e));
            self.value(name.text().into(), r, r, SymbolKind::Variable, false, value);
        }
    }

    fn local_function(&mut self, node: &SyntaxNode) {
        let Some((func, body)) = statement_function(node) else { return };
        match token(node, NAME) {
            Some(name) => {
                let r = name.text_range();
                let value = Value::Function { range: func, body };
                self.value(name.text().into(), r, r, SymbolKind::Variable, false, Some(value));
            }
            None => self.anonymous_function(String::new(), String::new(), func, body.as_ref(), node),
        }
    }

    fn function_stat(&mut self, node: &SyntaxNode) {
        let Some((func, body)) = statement_function(node) else { return };
        let path: Vec<SyntaxToken> = node
            .children()
            .find(|c| c.kind() == FUNC_NAME)
            .map(|n| {
                n.descendants_with_tokens().filter_map(|e| e.into_token()).filter(|t| !t.kind().is_trivia()).collect()
            })
            .unwrap_or_default();
        let value = Some(Value::Function { range: func, body: body.clone() });
        let (Some(first), Some(last)) = (path.first(), path.last()) else {
            return self.anonymous_function(String::new(), String::new(), func, body.as_ref(), node);
        };
        if last.kind() != NAME {
            return self.anonymous_function(String::new(), String::new(), func, body.as_ref(), node);
        }
        let last_r = last.text_range();
        let whole = TextRange::new(first.text_range().start(), last_r.end());
        let name = clip_last_line(&self.text[whole]);
        match path.len().checked_sub(2).map(|i| path[i].kind()) {
            None => self.value(name, last_r, last_r, SymbolKind::Variable, false, value),
            Some(COLON) => self.value(name, last_r, whole, SymbolKind::Variable, true, value),
            Some(_) => self.value(name, last_r, last_r, SymbolKind::Variable, false, value),
        }
    }

    fn assign_stat(&mut self, node: &SyntaxNode) {
        let exprs = exprs(node);
        let Some(vars) = node.children().find(|c| c.kind() == VAR_LIST) else { return };
        for (i, var) in vars.children().enumerate() {
            let (name, sel) = match var.kind() {
                NAME_EXPR => match token(&var, NAME) {
                    Some(t) => (t.text().to_string(), t.text_range()),
                    None => continue,
                },
                // `a.b = v`; `a[k] = v` defines nothing.
                INDEX_EXPR => match (token(&var, DOT), token(&var, NAME), trimmed(&var)) {
                    (Some(_), Some(field), Some(r)) => {
                        let fr = field.text_range();
                        (clip_last_line(&self.text[TextRange::new(r.start(), fr.end())]), fr)
                    }
                    _ => continue,
                },
                _ => continue,
            };
            let value = value_at(&exprs, i).map(|e| self.value_of(e));
            self.value(name, sel, sel, SymbolKind::Variable, false, value);
        }
    }

    /// The fields of a table constructor, numbering positional ones as LuaLS does.
    fn fields(&mut self, table: &SyntaxNode) {
        let mut tindex = 0;
        for field in table.children() {
            match field.kind() {
                FIELD_NAMED | FIELD_SET => {
                    let Some(name) = token(&field, NAME) else { continue };
                    let r = name.text_range();
                    let value = field.children().next().map(|e| self.value_of(e));
                    self.value(name.text().into(), r, r, SymbolKind::Variable, false, value);
                }
                FIELD_INDEXED => {
                    let (index, value) = indexed_parts(&field);
                    let Some(r) = index.as_ref().and_then(trimmed) else { continue };
                    let name = format!("[{}]", clip_last_line(&self.text[r]));
                    let value = value.map(|e| self.value_of(e));
                    self.value(name, r, r, SymbolKind::Variable, false, value);
                }
                FIELD_POS => {
                    tindex += 1;
                    let Some(expr) = field.children().next() else { continue };
                    let Some(r) = trimmed(&expr) else { continue };
                    let value = Some(self.value_of(expr));
                    self.value(format!("[{tindex}]"), r, r, SymbolKind::Variable, false, value);
                }
                _ => {}
            }
        }
    }

    fn value_of(&mut self, expr: SyntaxNode) -> Value {
        if matches!(expr.kind(), FUNCTION_EXPR | TABLE_EXPR)
            && let Some(r) = trimmed(&expr)
        {
            self.used.insert((expr.kind(), r));
        }
        if expr.kind() == FUNCTION_EXPR
            && let Some(range) = trimmed(&expr)
        {
            let body = expr.children().find(|c| c.kind() == FUNC_BODY);
            return Value::Function { range, body };
        }
        Value::Expr(expr)
    }

    /// LuaLS `buildValue`: `range` and `sel` before the value is known; the value
    /// sets the kind and detail and stretches the range to its end.
    fn value(
        &mut self,
        name: String,
        mut range: TextRange,
        sel: TextRange,
        mut kind: SymbolKind,
        method: bool,
        value: Option<Value>,
    ) {
        let mut detail = String::new();
        let mut value_range = None;
        let value_end = match value {
            Some(Value::Function { range: func, body }) => {
                detail = format!("function ({})", self.params(body.as_ref()));
                kind = if method { SymbolKind::Method } else { SymbolKind::Function };
                value_range = Some(func);
                range = TextRange::new(range.start().min(func.start()), range.end());
                Some(func.end())
            }
            Some(Value::Expr(expr)) => match trimmed(&expr) {
                Some(r) => {
                    match expr.kind() {
                        LITERAL_EXPR | UNARY_EXPR => {
                            if let Some((k, d)) = self.literal(&expr) {
                                kind = k;
                                detail = d;
                            }
                        }
                        TABLE_EXPR => {
                            (kind, detail) = self.table_detail(&expr);
                            value_range = Some(r);
                        }
                        CALL_EXPR | METHOD_CALL_EXPR => value_range = Some(r),
                        _ => {}
                    }
                    Some(r.end())
                }
                None => None,
            },
            None => None,
        };
        if let Some(end) = value_end {
            range = TextRange::new(range.start(), end.max(range.start()));
        }
        self.out.push(DocumentSymbol {
            name,
            detail,
            kind,
            range: range.cover(sel),
            selection_range: sel,
            value_range,
            children: Vec::new(),
        });
    }

    /// A function or table that is no definition's value.
    fn anonymous(&mut self, node: &SyntaxNode) {
        let Some(range) = trimmed(node) else { return };
        if !self.used.insert((node.kind(), range)) {
            return;
        }
        let (head, prefix) = match node.parent() {
            Some(p) if p.kind() == EXPR_LIST && p.parent().is_some_and(|s| s.kind() == RETURN_STAT) => {
                ("return".to_string(), "")
            }
            Some(p) if p.kind() == ARG_LIST => (p.parent().map(|call| self.callee(&call)).unwrap_or_default(), "-> "),
            _ => (String::new(), ""),
        };
        if node.kind() == FUNCTION_EXPR {
            let body = node.children().find(|c| c.kind() == FUNC_BODY);
            self.anonymous_function(head, prefix.into(), range, body.as_ref(), node);
        } else {
            let (kind, detail) = self.table_detail(node);
            self.out.push(DocumentSymbol {
                name: head,
                detail: format!("{prefix}{detail}"),
                kind,
                range,
                selection_range: range,
                value_range: Some(range),
                children: Vec::new(),
            });
        }
    }

    /// `node` holds the `function` keyword, the selection.
    fn anonymous_function(
        &mut self,
        name: String,
        prefix: String,
        range: TextRange,
        body: Option<&SyntaxNode>,
        node: &SyntaxNode,
    ) {
        let sel = token(node, FUNCTION_KW).map_or(range, |t| t.text_range());
        let detail = format!("{prefix}function ({})", self.params(body));
        self.out.push(DocumentSymbol {
            name,
            detail,
            kind: SymbolKind::Function,
            range,
            selection_range: sel,
            value_range: Some(range),
            children: Vec::new(),
        });
    }

    /// The called expression as written; for `obj:m(...)`, `obj:m`.
    fn callee(&self, call: &SyntaxNode) -> String {
        let Some(start) = trimmed(call).map(|r| r.start()) else { return String::new() };
        let end = match call.kind() {
            METHOD_CALL_EXPR => token(call, NAME).map(|t| t.text_range().end()),
            CALL_EXPR => call.children().next().as_ref().and_then(trimmed).map(|r| r.end()),
            _ => None,
        };
        end.map_or_else(String::new, |end| clip_last_line(&self.text[TextRange::new(start, end.max(start))]))
    }

    fn params(&self, body: Option<&SyntaxNode>) -> String {
        let Some(list) = body.and_then(|b| b.children().find(|c| c.kind() == PARAM_LIST)) else { return String::new() };
        let names: Vec<&str> = list
            .children()
            .filter(|c| c.kind() == PARAM)
            .map(|p| {
                if token(&p, DOT3).is_some() {
                    "..."
                } else {
                    token(&p, NAME).map_or("", |t| &self.text[t.text_range()])
                }
            })
            .collect();
        cap(&names.join(", "))
    }

    /// Kind and detail of a literal value; `-1` is a number as in LuaLS.
    fn literal(&self, expr: &SyntaxNode) -> Option<(SymbolKind, String)> {
        if expr.kind() == UNARY_EXPR {
            token(expr, MINUS)?;
            let inner = expr.children().next().filter(|c| c.kind() == LITERAL_EXPR)?;
            let number = token(&inner, NUMBER)?;
            return Some((SymbolKind::Number, cap(&format!("-{}", view_number(number.text())))));
        }
        let t = tokens(expr).find(|t| !t.kind().is_trivia())?;
        Some(match t.kind() {
            TRUE_KW | FALSE_KW => (SymbolKind::Boolean, t.text().into()),
            NUMBER => (SymbolKind::Number, cap(&view_number(t.text()))),
            HASH_STRING => (SymbolKind::Number, cap(t.text())),
            STRING | LONG_STRING => (SymbolKind::String, cap(&view_string(t.text(), t.kind()))),
            _ => return None,
        })
    }

    /// LuaLS `buildTable` / `buildArray`: the first five keys or values.
    fn table_detail(&self, table: &SyntaxNode) -> (SymbolKind, String) {
        let is_field = |c: &SyntaxNode| matches!(c.kind(), FIELD_POS | FIELD_NAMED | FIELD_INDEXED | FIELD_SET);
        let mut count = 0usize;
        let mut positional = 0usize;
        let mut last_positional = false;
        for field in table.children().filter(is_field) {
            count += 1;
            last_positional = field.kind() == FIELD_POS;
            positional += usize::from(last_positional);
        }
        if count == 0 {
            return (SymbolKind::Object, String::new());
        }
        let array = last_positional && positional == count;
        let mut parts = Vec::new();
        let mut tindex = 0;
        for field in table.children().filter(is_field).take(5) {
            match field.kind() {
                _ if array => {
                    if let Some(r) = trimmed(&field) {
                        parts.push(cap(&self.text[r]));
                    }
                }
                FIELD_NAMED | FIELD_SET => {
                    if let Some(name) = token(&field, NAME) {
                        parts.push(name.text().to_string());
                    }
                }
                FIELD_INDEXED => {
                    if let Some(r) = indexed_parts(&field).0.as_ref().and_then(trimmed) {
                        parts.push(format!("[{}]", cap(&self.text[r])));
                    }
                }
                _ => {
                    tindex += 1;
                    parts.push(format!("[{tindex}]"));
                }
            }
        }
        if count > 5 {
            parts.push(format!("...(+{})", count - 5));
        }
        let (kind, open, close) = if array { (SymbolKind::Array, '[', ']') } else { (SymbolKind::Object, '{', '}') };
        (kind, cap(&format!("{open}{}{close}", parts.join(", "))))
    }

    fn if_stat(&mut self, node: &SyntaxNode) {
        for clause in if_clauses(node) {
            let start = clause.range.start();
            let detail_end = token(&clause.owner, THEN_KW).unwrap_or_else(|| clause.keyword.clone()).text_range().end();
            self.out.push(DocumentSymbol {
                name: clause.keyword.text().to_string(),
                detail: cap(&self.text[TextRange::new(start, detail_end.max(start))]),
                kind: SymbolKind::Package,
                range: clause.range,
                selection_range: clause.keyword.text_range(),
                value_range: Some(clause.range),
                children: Vec::new(),
            });
        }
    }

    /// `while` and `for`: the detail runs from the keyword to `do`.
    fn block(&mut self, name: &str, node: &SyntaxNode, keyword: SyntaxKind, end_kw: SyntaxKind) {
        let (Some(range), Some(kw)) = (trimmed(node), token(node, keyword)) else { return };
        let detail_end = token(node, end_kw).unwrap_or_else(|| kw.clone()).text_range().end();
        self.out.push(DocumentSymbol {
            name: name.into(),
            detail: cap(&self.text[TextRange::new(range.start(), detail_end.max(range.start()))]),
            kind: SymbolKind::Package,
            range,
            selection_range: kw.text_range(),
            value_range: Some(range),
            children: Vec::new(),
        });
    }

    fn repeat_stat(&mut self, node: &SyntaxNode) {
        let (Some(range), Some(kw)) = (trimmed(node), token(node, REPEAT_KW)) else { return };
        let detail = match (token(node, UNTIL_KW), node.children().filter(|c| c.kind() != BLOCK).last()) {
            (Some(until), Some(cond)) => match trimmed(&cond) {
                Some(c) => cap(&self.text[TextRange::new(until.text_range().start(), c.end())]),
                None => String::new(),
            },
            _ => String::new(),
        };
        self.out.push(DocumentSymbol {
            name: "repeat".into(),
            detail,
            kind: SymbolKind::Package,
            range,
            selection_range: kw.text_range(),
            value_range: Some(range),
            children: Vec::new(),
        });
    }
}

/// LuaLS `packChild`, without recursion: after a stable sort by value start, each
/// symbol goes under the innermost open symbol whose value holds its selection.
fn nest(mut flat: Vec<DocumentSymbol>) -> Vec<DocumentSymbol> {
    flat.sort_by_key(|s| s.value_range.unwrap_or(s.selection_range).start());
    let mut root = Vec::new();
    let mut open: Vec<DocumentSymbol> = Vec::new();
    fn attach(open: &mut [DocumentSymbol], root: &mut Vec<DocumentSymbol>, s: DocumentSymbol) {
        match open.last_mut() {
            Some(parent) => parent.children.push(s),
            None => root.push(s),
        }
    }
    for sym in flat {
        while let Some(top) = open.last() {
            if top.value_range.is_some_and(|v| v.contains_range(sym.selection_range)) {
                break;
            }
            let done = open.pop().expect("non-empty");
            attach(&mut open, &mut root, done);
        }
        if sym.value_range.is_some() {
            open.push(sym);
        } else {
            attach(&mut open, &mut root, sym);
        }
    }
    while let Some(done) = open.pop() {
        attach(&mut open, &mut root, done);
    }
    root
}

fn exprs(stat: &SyntaxNode) -> Vec<SyntaxNode> {
    stat.children().find(|c| c.kind() == EXPR_LIST).map(|l| l.children().collect()).unwrap_or_default()
}

/// The `i`th value; past the list, the last one when it returns several (a call or `...`).
fn value_at(exprs: &[SyntaxNode], i: usize) -> Option<SyntaxNode> {
    exprs
        .get(i)
        .cloned()
        .or_else(|| exprs.last().filter(|e| matches!(e.kind(), CALL_EXPR | METHOD_CALL_EXPR | VARARG_EXPR)).cloned())
}

/// `[index] = value`: the expression before `=` and the one after.
fn indexed_parts(field: &SyntaxNode) -> (Option<SyntaxNode>, Option<SyntaxNode>) {
    let eq = token(field, EQ).map(|t| t.text_range().start());
    let mut index = None;
    let mut value = None;
    for child in field.children() {
        match eq {
            Some(eq) if child.text_range().start() >= eq => {
                value.get_or_insert(child);
            }
            _ => {
                index.get_or_insert(child);
            }
        }
    }
    (index, value)
}

fn cap(s: &str) -> String {
    if s.len() <= MAX_TEXT {
        return s.to_string();
    }
    let mut end = MAX_TEXT;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

/// LuaLS `clipLastLine`: a name over several lines shows its last line. Only the
/// end of a long text is searched, so chained calls in a minified file stay linear.
fn clip_last_line(s: &str) -> String {
    const WINDOW: usize = 1024;
    let mut from = s.len().saturating_sub(WINDOW);
    while !s.is_char_boundary(from) {
        from += 1;
    }
    match s[from..].rfind(['\r', '\n']) {
        Some(i) => format!("... {}", cap(s[from + i + 1..].trim())),
        None if from > 0 => format!("... {}", cap(s[from..].trim())),
        None => cap(s),
    }
}

/// LuaLS `viewLiteral` for numbers: integers in decimal, floats with up to ten
/// decimals and no trailing zeros.
fn view_number(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    if let Some(hex) = lower.strip_prefix("0x") {
        if hex.is_empty() || hex.contains(['.', 'p']) {
            return raw.to_string();
        }
        // Lua hex integers wrap around.
        let mut v: u64 = 0;
        for c in hex.chars() {
            let Some(d) = c.to_digit(16) else { return raw.to_string() };
            v = v.wrapping_mul(16).wrapping_add(u64::from(d));
        }
        return (v as i64).to_string();
    }
    if !lower.contains(['.', 'e'])
        && let Ok(i) = lower.parse::<i64>()
    {
        return i.to_string();
    }
    match lower.parse::<f64>() {
        Ok(f) if f.is_finite() => {
            let s = format!("{f:.10}");
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        }
        Ok(f) => if f > 0.0 { "1e9999" } else { "-1e9999" }.to_string(),
        Err(_) => raw.to_string(),
    }
}

/// LuaLS `viewString` of the string's value: `'x'` shows as `"x"`.
fn view_string(raw: &str, kind: SyntaxKind) -> String {
    let value = if kind == LONG_STRING { long_string_value(raw) } else { short_string_value(raw) };
    let Some(value) = value.and_then(|v| String::from_utf8(v).ok()) else { return raw.to_string() };
    let quote = if value.contains(['\r', '\n']) {
        None
    } else if !value.contains('\'') && value.contains('"') {
        Some('\'')
    } else {
        Some('"')
    };
    let control = |c: char| matches!(c, '\0'..='\x08' | '\x0b'..='\x0c' | '\x0e'..='\x1f' | '\x7f');
    match quote {
        Some(q) => {
            let mut out = String::with_capacity(value.len() + 2);
            out.push(q);
            for c in value.chars() {
                match c {
                    _ if c == q => {
                        out.push('\\');
                        out.push(c);
                    }
                    _ if control(c) => out.push_str(&format!("\\{:03}", c as u32)),
                    _ => out.push(c),
                }
            }
            out.push(q);
            out
        }
        None => {
            let value: String = value.chars().filter(|&c| !control(c)).collect();
            for level in 0..=10 {
                let close = format!("]{}]", "=".repeat(level));
                if !value.contains(&close) {
                    return format!("[{}[{value}{close}", "=".repeat(level));
                }
            }
            raw.to_string()
        }
    }
}

/// The bytes of a quoted string, escapes resolved; `None` on an escape Lua rejects.
fn short_string_value(raw: &str) -> Option<Vec<u8>> {
    let bytes = raw.as_bytes();
    let q = *bytes.first()?;
    let inner = if bytes.len() >= 2 && bytes[bytes.len() - 1] == q { &bytes[1..bytes.len() - 1] } else { &bytes[1..] };
    let mut out = Vec::with_capacity(inner.len());
    let mut i = 0;
    while i < inner.len() {
        let c = inner[i];
        i += 1;
        if c != b'\\' {
            out.push(c);
            continue;
        }
        let e = *inner.get(i)?;
        i += 1;
        match e {
            b'n' => out.push(b'\n'),
            b't' => out.push(b'\t'),
            b'r' => out.push(b'\r'),
            b'a' => out.push(7),
            b'b' => out.push(8),
            b'f' => out.push(12),
            b'v' => out.push(11),
            b'\\' | b'"' | b'\'' => out.push(e),
            b'\n' | b'\r' => {
                out.push(b'\n');
                if inner.get(i).is_some_and(|&n| (n == b'\n' || n == b'\r') && n != e) {
                    i += 1;
                }
            }
            b'x' => {
                let hex = std::str::from_utf8(inner.get(i..i + 2)?).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 2;
            }
            b'z' => {
                while inner.get(i).is_some_and(|b| b.is_ascii_whitespace()) {
                    i += 1;
                }
            }
            b'0'..=b'9' => {
                let mut v = u32::from(e - b'0');
                for _ in 0..2 {
                    match inner.get(i) {
                        Some(d) if d.is_ascii_digit() => {
                            v = v * 10 + u32::from(d - b'0');
                            i += 1;
                        }
                        _ => break,
                    }
                }
                out.push(u8::try_from(v).ok()?);
            }
            b'u' => {
                if inner.get(i) != Some(&b'{') {
                    return None;
                }
                let close = i + inner[i..].iter().position(|&b| b == b'}')?;
                let code = u32::from_str_radix(std::str::from_utf8(&inner[i + 1..close]).ok()?, 16).ok()?;
                let mut buf = [0; 4];
                out.extend_from_slice(char::from_u32(code)?.encode_utf8(&mut buf).as_bytes());
                i = close + 1;
            }
            _ => return None,
        }
    }
    Some(out)
}

/// `[==[ ... ]==]` without its brackets and the line break right after the opening.
fn long_string_value(raw: &str) -> Option<Vec<u8>> {
    let level = raw.get(1..)?.bytes().take_while(|&b| b == b'=').count();
    let open = level + 2;
    let close = format!("]{}]", "=".repeat(level));
    let body = raw.get(open..)?;
    let body = body.strip_suffix(close.as_str()).unwrap_or(body);
    let body = ["\r\n", "\n\r", "\n", "\r"].iter().find_map(|nl| body.strip_prefix(nl)).unwrap_or(body);
    Some(body.as_bytes().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    fn symbols(text: &str) -> Vec<DocumentSymbol> {
        document_symbols(&parse(text), text)
    }

    #[test]
    fn literal_views_follow_luals() {
        assert_eq!(view_number("0x10"), "16");
        assert_eq!(view_number("1.0"), "1");
        assert_eq!(view_number("1.25"), "1.25");
        assert_eq!(view_number("1e3"), "1000");
        assert_eq!(view_number("10"), "10");
        assert_eq!(view_string("'x'", STRING), "\"x\"");
        assert_eq!(view_string("'a\"b'", STRING), "'a\"b'");
        assert_eq!(view_string("\"a\\tb\\65\"", STRING), "\"a\tbA\"");
        assert_eq!(view_string("[[\nline1\nline2]]", LONG_STRING), "[[line1\nline2]]");
        assert_eq!(view_string("'\\xff'", STRING), "'\\xff'");
    }

    #[test]
    fn cfxlua_shapes() {
        let s = symbols("local t = { .a, .b = 1 }\nx += 1\nlocal h = `name`\n");
        assert_eq!(s.len(), 2, "{s:#?}");
        assert_eq!(s[0].detail, "{a, b}");
        assert_eq!(s[0].children.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!((s[1].kind, s[1].detail.as_str()), (SymbolKind::Number, "`name`"));
    }

    #[test]
    fn comments_stay_out_of_ranges() {
        let text = "-- about f\nlocal function f() end -- trailing\n";
        let s = symbols(text);
        assert_eq!(&text[s[0].range], "function f() end");
    }

    #[test]
    fn long_text_is_cut() {
        let long = format!("local t = {{ {} }}", "1 + ".repeat(200) + "1");
        let s = symbols(&long);
        assert!(s[0].detail.len() <= MAX_TEXT + 8, "{}", s[0].detail.len());
        // A callee spread over a long minified line stays bounded too.
        let chain = format!("f{}", "(function() end)".repeat(300));
        assert!(symbols(&chain).iter().all(|s| s.name.len() <= MAX_TEXT + 8));
    }

    #[test]
    fn long_call_chains_stay_linear() {
        // Each link names its callee, the whole chain before it. Four times the links
        // must cost about four times as much, not sixteen (a quadratic walk took 6 s
        // in release on 20,000 links). The symbol pass alone is timed, best of three.
        let time = |links: usize| {
            let text = format!("f{}", "(function() end)".repeat(links));
            let parse = parse(&text);
            (0..3)
                .map(|_| {
                    let start = std::time::Instant::now();
                    assert_eq!(document_symbols(&parse, &text).len(), links);
                    start.elapsed()
                })
                .min()
                .unwrap()
        };
        let (small, large) = (time(2_000), time(8_000));
        let ratio = large.as_secs_f64() / small.as_secs_f64();
        assert!(ratio < 7.0, "4x links took {ratio:.1}x the time ({small:?} -> {large:?})");
    }
}
