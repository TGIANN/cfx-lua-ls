//! The literal a value is known to hold, as LuaLS shows it after `=` on
//! hover (`local y: integer = 1` for `local y = x`): literals followed through
//! locals, calls of the file's functions, constant operators and indexes into
//! constructors.

use crate::names::{Access, Names, Target};
use crate::TextRange;
use cfx_lua_syntax::SyntaxKind::*;
use cfx_lua_syntax::SyntaxNode;

const MAX_DEPTH: u32 = 12;

#[derive(Clone)]
enum Lit {
    Int(i64),
    Num(f64),
    /// Content, and the quote of the literal written in the source (`'`,
    /// `"`, `[[`).
    Str(String, Option<String>),
    Bool(bool),
}

impl Lit {
    /// LuaLS `util.viewLiteral` / `viewString`.
    fn shown(&self) -> String {
        match self {
            Lit::Int(i) => i.to_string(),
            Lit::Num(n) if n.fract() == 0.0 && n.abs() < 1e15 => format!("{}", *n as i64),
            Lit::Num(n) => {
                let s = format!("{n:.10}");
                s.trim_end_matches('0').trim_end_matches('.').to_string()
            }
            Lit::Str(s, q) => view_string(s, q.as_deref()),
            Lit::Bool(b) => b.to_string(),
        }
    }
}

fn view_string(s: &str, quote: Option<&str>) -> String {
    let q = match quote {
        Some(q) => q,
        None if s.contains(['\r', '\n']) => "[[",
        None if !s.contains('\'') && s.contains('"') => "'",
        None => "\"",
    };
    if q.starts_with('[') {
        let close = q.replace('[', "]");
        let body: String = s.chars().filter(|c| !is_control(*c)).collect();
        return format!("{q}{body}{close}");
    }
    let mut out = String::from(q);
    for c in s.chars() {
        match c {
            '\r' => out.push_str("\\r"),
            '\n' => out.push_str("\\\n"),
            c if q.starts_with(c) => {
                out.push('\\');
                out.push(c);
            }
            c if is_control(c) => out.push_str(&format!("\\{:03}", c as u32)),
            c => out.push(c),
        }
    }
    out.push_str(q);
    out
}

fn is_control(c: char) -> bool {
    matches!(c as u32, 0..=8 | 11..=12 | 14..=31 | 127)
}

struct Cx<'a> {
    root: SyntaxNode,
    names: &'a Names,
    text: &'a str,
}

/// The literals the local declared at `decl` is given (its value and every
/// assignment), joined as LuaLS shows them: `true|false`, `1`, `'a'|'b'`.
pub(crate) fn decl_value(root: &SyntaxNode, names: &Names, text: &str, decl: usize) -> Option<String> {
    let cx = Cx { root: root.clone(), names, text };
    let mut shown: Vec<String> = Vec::new();
    for l in cx.decl_all(decl, 0) {
        let s = l.shown();
        if !shown.contains(&s) {
            shown.push(s);
        }
    }
    sort_literals(&mut shown);
    (!shown.is_empty()).then(|| shown.join("|"))
}

/// The literals a list of values gives, joined as `decl_value` joins them.
pub(crate) fn literals_shown(root: &SyntaxNode, names: &Names, text: &str, values: &[SyntaxNode]) -> Option<String> {
    let cx = Cx { root: root.clone(), names, text };
    let mut shown: Vec<String> = Vec::new();
    for v in values {
        if let Some(l) = cx.eval(v, 0) {
            let s = l.shown();
            if !shown.contains(&s) {
                shown.push(s);
            }
        }
    }
    sort_literals(&mut shown);
    (!shown.is_empty()).then(|| shown.join("|"))
}

fn sort_literals(shown: &mut [String]) {
    let rank = |s: &str| match s {
        "true" => 1,
        "false" => 2,
        _ => 0,
    };
    shown.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.cmp(b)));
}

/// The value of a literal expression as LuaLS shows it (`0xff` is `255`).
pub(crate) fn literal_shown(root: &SyntaxNode, names: &Names, text: &str, e: &SyntaxNode) -> Option<String> {
    let cx = Cx { root: root.clone(), names, text };
    cx.eval(e, 0).map(|l| l.shown())
}

impl Cx<'_> {
    fn node_at(&self, r: TextRange) -> Option<SyntaxNode> {
        match self.root.covering_element(r) {
            rowan::NodeOrToken::Node(n) => Some(n),
            rowan::NodeOrToken::Token(t) => t.parent(),
        }
    }

    /// The `i`th value of an expression list, a call at its end giving the rest.
    fn nth_value(&self, list: &SyntaxNode, i: usize, depth: u32) -> Option<Lit> {
        let exprs: Vec<SyntaxNode> = list.children().collect();
        if let Some(e) = exprs.get(i)
            && (i + 1 < exprs.len() || !matches!(e.kind(), CALL_EXPR | METHOD_CALL_EXPR))
        {
            return self.eval(e, depth);
        }
        let last = exprs.last()?;
        if matches!(last.kind(), CALL_EXPR) {
            return self.call(last, i + 1 - exprs.len(), depth);
        }
        None
    }

    /// The single literal a local holds, when all it is given is one value.
    fn decl(&self, di: usize, depth: u32) -> Option<Lit> {
        let mut all = self.decl_all(di, depth);
        let first = all.pop()?;
        all.iter().all(|l| l.shown() == first.shown()).then_some(first)
    }

    /// Every literal a local is given: its value and its assignments.
    fn decl_all(&self, di: usize, depth: u32) -> Vec<Lit> {
        let mut out = Vec::new();
        if depth > MAX_DEPTH {
            return out;
        }
        let Some(d) = self.names.decls.get(di) else { return out };
        let Some(name) = self.node_at(d.range) else { return out };
        // `local a, b = ...`
        let Some(stat) = name.ancestors().find(|a| a.kind() == LOCAL_STAT).filter(|s| s.text_range() == d.owner) else { return out };
        let Some(i) = stat.children().filter(|c| c.kind() == LOCAL_NAME).position(|n| n.text_range().contains_range(d.range)) else { return out };
        if let Some(list) = stat.children().find(|c| c.kind() == EXPR_LIST) {
            out.extend(self.nth_values(&list, i, depth + 1));
        }
        for w in self.names.refs.iter().filter(|r| r.target == Target::Decl(di) && r.access == Access::Write) {
            let Some(var) = self.node_at(w.range).and_then(|n| n.ancestors().find(|a| a.kind() == NAME_EXPR)) else { continue };
            let Some(vars) = var.parent().filter(|p| p.kind() == VAR_LIST) else { continue };
            let Some(i) = vars.children().position(|v| v == var) else { continue };
            if let Some(list) = vars.parent().and_then(|s| s.children().find(|c| c.kind() == EXPR_LIST)) {
                out.extend(self.nth_values(&list, i, depth + 1));
            }
        }
        out
    }

    /// The literals the `i`th value of a list may be (a local's every value).
    fn nth_values(&self, list: &SyntaxNode, i: usize, depth: u32) -> Vec<Lit> {
        let exprs: Vec<SyntaxNode> = list.children().collect();
        if let Some(e) = exprs.get(i)
            && e.kind() == NAME_EXPR
            && let Some(tok) = e.first_token()
            && let Some(r) = self.names.refs.iter().find(|r| r.range == tok.text_range())
            && let Target::Decl(di) = r.target
        {
            return self.decl_all(di, depth + 1);
        }
        self.nth_value(list, i, depth).into_iter().collect()
    }

    fn eval(&self, e: &SyntaxNode, depth: u32) -> Option<Lit> {
        if depth > MAX_DEPTH {
            return None;
        }
        match e.kind() {
            LITERAL_EXPR => {
                let t = e.first_token()?;
                match t.kind() {
                    NUMBER => {
                        let s = t.text();
                        match parse_int(s) {
                            Some(i) => Some(Lit::Int(i)),
                            None => s.parse().ok().map(Lit::Num),
                        }
                    }
                    STRING => {
                        let raw = t.text();
                        // `[==[`: up to the second `[`.
                        let q = match raw.strip_prefix('[') {
                            Some(r) => format!("[{}[", "=".repeat(r.bytes().take_while(|&b| b == b'=').count())),
                            None => raw[..1].to_string(),
                        };
                        Some(Lit::Str(crate::util::lua_string(raw), Some(q)))
                    }
                    TRUE_KW => Some(Lit::Bool(true)),
                    FALSE_KW => Some(Lit::Bool(false)),
                    _ => None,
                }
            }
            PAREN_EXPR => self.eval(&e.first_child()?, depth + 1),
            NAME_EXPR => {
                let tok = e.first_token()?;
                let r = self.names.refs.iter().find(|r| r.range == tok.text_range())?;
                match r.target {
                    Target::Decl(di) => self.decl(di, depth + 1),
                    Target::Global => None,
                }
            }
            UNARY_EXPR => {
                let op = e.first_token()?;
                let inner = e.first_child()?;
                match op.kind() {
                    MINUS => match self.eval(&inner, depth + 1)? {
                        Lit::Int(i) => Some(Lit::Int(i.checked_neg()?)),
                        Lit::Num(n) => Some(Lit::Num(-n)),
                        _ => None,
                    },
                    NOT_KW => match self.eval(&inner, depth + 1)? {
                        Lit::Bool(b) => Some(Lit::Bool(!b)),
                        _ => Some(Lit::Bool(false)),
                    },
                    HASH => {
                        let t = self.table(&inner, depth + 1)?;
                        Some(Lit::Int(border(self, &t, depth)))
                    }
                    _ => None,
                }
            }
            BINARY_EXPR => {
                let mut kids = e.children();
                let (a, b) = (kids.next()?, kids.next()?);
                let op = e.children_with_tokens().filter_map(|x| x.into_token()).find(|t| !t.kind().is_trivia())?;
                let (a, b) = (self.eval(&a, depth + 1)?, self.eval(&b, depth + 1)?);
                if op.kind() == DOT2 {
                    let part = |l: &Lit| match l {
                        Lit::Str(s, _) => Some(s.clone()),
                        Lit::Int(_) | Lit::Num(_) => Some(l.shown()),
                        Lit::Bool(_) => None,
                    };
                    return Some(Lit::Str(format!("{}{}", part(&a)?, part(&b)?), None));
                }
                let (Lit::Int(x), Lit::Int(y)) = (a, b) else { return None };
                Some(Lit::Int(match op.kind() {
                    PLUS => x.wrapping_add(y),
                    MINUS => x.wrapping_sub(y),
                    STAR => x.wrapping_mul(y),
                    SLASH2 if y != 0 => x.div_euclid(y),
                    PERCENT if y != 0 => x.rem_euclid(y),
                    SHL => shift(x, y),
                    SHR => shift(x, -y),
                    AMP => x & y,
                    PIPE => x | y,
                    TILDE => x ^ y,
                    _ => return None,
                }))
            }
            INDEX_EXPR => {
                let t = self.table(&e.first_child()?, depth + 1)?;
                let key = match crate::lint::field_name(e) {
                    Some(n) => Lit::Str(n.text().to_string(), None),
                    None => self.eval(&e.children().nth(1)?, depth + 1)?,
                };
                self.entry(&t, &key, depth + 1)
            }
            CALL_EXPR => self.call(e, 0, depth + 1),
            _ => None,
        }
    }

    /// The table constructor an expression holds.
    fn table(&self, e: &SyntaxNode, depth: u32) -> Option<SyntaxNode> {
        if depth > MAX_DEPTH {
            return None;
        }
        match e.kind() {
            TABLE_EXPR => Some(e.clone()),
            PAREN_EXPR => self.table(&e.first_child()?, depth + 1),
            NAME_EXPR => {
                let tok = e.first_token()?;
                let r = self.names.refs.iter().find(|r| r.range == tok.text_range())?;
                let Target::Decl(di) = r.target else { return None };
                let d = &self.names.decls[di];
                let stat = self.node_at(d.range)?.ancestors().find(|a| a.kind() == LOCAL_STAT)?;
                let i = stat.children().filter(|c| c.kind() == LOCAL_NAME).position(|n| n.text_range().contains_range(d.range))?;
                let v = stat.children().find(|c| c.kind() == EXPR_LIST)?.children().nth(i)?;
                self.table(&v, depth + 1)
            }
            _ => None,
        }
    }

    /// The value under `key` in a constructor.
    fn entry(&self, t: &SyntaxNode, key: &Lit, depth: u32) -> Option<Lit> {
        let mut pos = 0;
        for f in t.children() {
            match f.kind() {
                FIELD_POS => {
                    pos += 1;
                    if matches!(key, Lit::Int(i) if *i == pos) {
                        return self.eval(&f.first_child()?, depth + 1);
                    }
                }
                FIELD_NAMED => {
                    let name = f.children_with_tokens().filter_map(|x| x.into_token()).find(|t| t.kind() == NAME)?;
                    if matches!(key, Lit::Str(s, _) if s == name.text()) {
                        return self.eval(&f.children().last()?, depth + 1);
                    }
                }
                FIELD_INDEXED => {
                    let mut kids = f.children();
                    let k = self.eval(&kids.next()?, depth + 1);
                    let same = match (&k, key) {
                        (Some(Lit::Int(a)), Lit::Int(b)) => a == b,
                        (Some(Lit::Str(a, _)), Lit::Str(b, _)) => a == b,
                        _ => false,
                    };
                    if same {
                        return self.eval(&kids.next()?, depth + 1);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// The `i`th value a call returns: `pcall(f, ...)`'s after `true`, a
    /// function of the file's first `return`, a `---@return T` bound to the
    /// argument of a `---@param x T`.
    fn call(&self, call: &SyntaxNode, i: usize, depth: u32) -> Option<Lit> {
        if depth > MAX_DEPTH {
            return None;
        }
        let callee = call.first_child().filter(|c| c.kind() == NAME_EXPR)?;
        let args: Vec<SyntaxNode> = call.children().find(|c| c.kind() == ARG_LIST)?.children().collect();
        let tok = callee.first_token()?;
        let r = self.names.refs.iter().find(|r| r.range == tok.text_range())?;
        let body = match r.target {
            Target::Global if tok.text() == "pcall" => {
                if i == 0 {
                    return Some(Lit::Bool(true));
                }
                let f = args.first()?.clone();
                // `pcall(f, a)` as `f(a)`.
                let inner = f.first_token()?;
                let fr = self.names.refs.iter().find(|r| r.range == inner.text_range())?;
                return self.function_value(fr.target, inner.text(), &args[1..], i - 1, depth + 1);
            }
            t => (t, tok.text()),
        };
        self.function_value(body.0, body.1, &args, i, depth + 1)
    }

    fn function_value(&self, target: Target, name: &str, args: &[SyntaxNode], i: usize, depth: u32) -> Option<Lit> {
        let stat = match target {
            Target::Decl(di) => {
                let d = &self.names.decls[di];
                self.node_at(d.owner)?
            }
            Target::Global => self.root.descendants().find(|s| {
                s.kind() == FUNCTION_STAT && s.children().find(|c| c.kind() == FUNC_NAME).is_some_and(|n| n.text() == name)
            })?,
        };
        let body = stat.descendants().find(|n| n.kind() == FUNC_BODY)?;
        // `---@generic T` / `---@param x T` / `---@return T`.
        let doc: Vec<&str> = self.text[stat.text_range()].lines().map(str::trim_start).take_while(|l| l.starts_with("--")).collect();
        let generics: Vec<&str> = doc.iter().filter_map(|l| l.strip_prefix("---@generic")).flat_map(|r| r.split([',', ' ']).filter(|g| !g.is_empty())).collect();
        if !generics.is_empty() {
            let ret = doc.iter().filter_map(|l| l.strip_prefix("---@return")).nth(i).and_then(|r| r.split_whitespace().next());
            if let Some(ret) = ret.filter(|r| generics.contains(r)) {
                let params: Vec<String> = body
                    .children()
                    .find(|c| c.kind() == PARAM_LIST)
                    .map(|l| l.children().filter(|p| p.kind() == PARAM).map(|p| p.text().to_string()).collect())
                    .unwrap_or_default();
                let p = doc.iter().filter_map(|l| l.strip_prefix("---@param")).find_map(|r| {
                    let mut w = r.split_whitespace();
                    let (n, t) = (w.next()?, w.next()?);
                    (t == ret).then_some(n)
                })?;
                let at = params.iter().position(|x| x == p)?;
                return self.eval(args.get(at)?, depth + 1);
            }
        }
        let block = body.children().find(|c| c.kind() == BLOCK)?;
        let ret = block.children().find(|c| c.kind() == RETURN_STAT)?;
        let list = ret.children().find(|c| c.kind() == EXPR_LIST)?;
        self.nth_value(&list, i, depth + 1)
    }
}

/// `#t` of a constructor: the last of the keys 1, 2, ... it holds.
fn border(cx: &Cx, t: &SyntaxNode, depth: u32) -> i64 {
    let mut n = 0;
    while cx.entry(t, &Lit::Int(n + 1), depth + 1).is_some() {
        n += 1;
    }
    n
}

fn shift(x: i64, by: i64) -> i64 {
    match by {
        b if b <= -64 || b >= 64 => 0,
        b if b >= 0 => ((x as u64) << b) as i64,
        b => ((x as u64) >> -b) as i64,
    }
}

fn parse_int(s: &str) -> Option<i64> {
    if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        return u64::from_str_radix(h, 16).ok().map(|v| v as i64);
    }
    s.parse().ok()
}
