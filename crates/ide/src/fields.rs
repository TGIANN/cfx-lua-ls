//! Field references inside one file. A field is named by its path from a local
//! or a global (`a.b.c`): `t.a`, `t['a']`, `{ a = 1 }` under `local t =` /
//! `t =` (nested constructors too), `function t.a()`, `function mt:x()` and
//! `self.x` / `self:x()` in its methods. No aliasing and no types: `local u = t`
//! starts a new path.

use crate::names::{DeclKind, Names, Target};
use cfx_lua_syntax::{Parse, SyntaxKind::*, SyntaxNode, SyntaxToken, TextRange, TextSize};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Base {
    Decl(usize),
    Global(String),
}

type Path = (Base, Vec<String>);

struct Site {
    /// The key: the name, or the string with its quotes.
    key: TextRange,
    /// What a rename replaces: the name, or the string's content.
    name: TextRange,
    /// The owner's path; the field is `name`'s text.
    owner: Path,
    field: String,
    write: bool,
    /// `t[expr]` with a key that is not a literal string.
    dynamic: bool,
}

struct Fields<'a> {
    root: SyntaxNode,
    names: &'a Names,
    refs: HashMap<TextSize, Target>,
    decls: HashMap<TextSize, usize>,
}

impl Fields<'_> {
    fn target(&self, t: &SyntaxToken) -> Option<Target> {
        self.refs.get(&t.text_range().start()).copied()
    }

    /// The path a name token starts: its local, or the method's object for
    /// `self`, or the global.
    fn base(&self, t: &SyntaxToken, depth: usize) -> Option<Path> {
        match self.target(t)? {
            Target::Global => Some((Base::Global(t.text().to_string()), Vec::new())),
            Target::Decl(d) if self.names.decls[d].kind == DeclKind::SelfParam => self.method_owner(d, depth),
            Target::Decl(d) => Some((Base::Decl(d), Vec::new())),
        }
    }

    /// `function a.b:m()`: the path of `a.b`, for `self` in it.
    fn method_owner(&self, d: usize, depth: usize) -> Option<Path> {
        if depth > 8 {
            return None;
        }
        let colon = self.root.token_at_offset(self.names.decls[d].range.start()).right_biased()?;
        let fname = colon.parent().filter(|p| p.kind() == FUNC_NAME)?;
        let parts: Vec<SyntaxToken> = crate::util::tokens(&fname).filter(|t| t.kind() == NAME).collect();
        let (_, init) = parts.split_last()?;
        let (first, rest) = init.split_first()?;
        let (base, mut path) = self.base(first, depth + 1)?;
        path.extend(rest.iter().map(|t| t.text().to_string()));
        Some((base, path))
    }

    /// The value an expression names: `t`, `t.a`, `t['a'].b`.
    fn path(&self, e: &SyntaxNode) -> Option<Path> {
        match e.kind() {
            NAME_EXPR => self.base(&e.first_token()?, 0),
            INDEX_EXPR => {
                let (field, ..) = key(e)?;
                let (base, mut path) = self.path(&e.first_child()?)?;
                path.push(field);
                Some((base, path))
            }
            _ => None,
        }
    }

    /// The path of the value a table constructor is stored in.
    fn table_path(&self, table: &SyntaxNode) -> Option<Path> {
        let parent = table.parent()?;
        match parent.kind() {
            EXPR_LIST => {
                let i = parent.children().position(|c| c == *table)?;
                let st = parent.parent()?;
                match st.kind() {
                    LOCAL_STAT => {
                        let name = st.children().filter(|c| c.kind() == LOCAL_NAME).nth(i)?;
                        let d = *self.decls.get(&name.text_range().start())?;
                        Some((Base::Decl(d), Vec::new()))
                    }
                    ASSIGN_STAT => self.path(&st.children().find(|c| c.kind() == VAR_LIST)?.children().nth(i)?),
                    _ => None,
                }
            }
            FIELD_NAMED | FIELD_INDEXED => {
                let (field, ..) = table_key(&parent)?;
                let (base, mut path) = self.table_path(&parent.parent()?)?;
                path.push(field);
                Some((base, path))
            }
            _ => None,
        }
    }

    fn sites(&self) -> Vec<Site> {
        let mut out = Vec::new();
        for n in self.root.descendants() {
            match n.kind() {
                INDEX_EXPR => {
                    let Some(owner) = n.first_child().and_then(|o| self.path(&o)) else { continue };
                    let write = n.parent().is_some_and(|p| p.kind() == VAR_LIST);
                    match key(&n) {
                        Some((field, key, name)) => out.push(Site { key, name, owner, field, write, dynamic: false }),
                        None => {
                            let r = TextRange::empty(n.text_range().start());
                            out.push(Site { key: r, name: r, owner, field: String::new(), write, dynamic: true });
                        }
                    }
                }
                METHOD_CALL_EXPR => {
                    let (Some(owner), Some(t)) = (n.first_child().and_then(|o| self.path(&o)), crate::api::method_name(&n)) else { continue };
                    out.push(Site { key: t.text_range(), name: t.text_range(), owner, field: t.text().to_string(), write: false, dynamic: false });
                }
                FUNC_NAME => {
                    let parts: Vec<SyntaxToken> = crate::util::tokens(&n).filter(|t| t.kind() == NAME).collect();
                    let Some((last, init)) = parts.split_last() else { continue };
                    let Some((first, rest)) = init.split_first() else { continue };
                    let Some((base, mut path)) = self.base(first, 0) else { continue };
                    path.extend(rest.iter().map(|t| t.text().to_string()));
                    out.push(Site { key: last.text_range(), name: last.text_range(), owner: (base, path), field: last.text().to_string(), write: true, dynamic: false });
                }
                TABLE_EXPR => {
                    let Some(owner) = self.table_path(&n) else { continue };
                    for f in n.children().filter(|f| matches!(f.kind(), FIELD_NAMED | FIELD_INDEXED)) {
                        if let Some((field, key, name)) = table_key(&f) {
                            out.push(Site { key, name, owner: owner.clone(), field, write: true, dynamic: false });
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// Whether the local holds a table built here: `local t` or
    /// `local t = { ... }`, not a parameter, a loop variable, a call's result
    /// or CfxLua's `local a in t`.
    fn local_table(&self, d: usize) -> bool {
        let decl = &self.names.decls[d];
        if decl.kind != DeclKind::Local {
            return false;
        }
        let Some(st) = self.root.token_at_offset(decl.range.start()).right_biased().and_then(|t| t.parent_ancestors().find(|a| a.kind() == LOCAL_STAT)) else {
            return false;
        };
        if crate::util::tokens(&st).any(|t| t.kind() == IN_KW) {
            return false;
        }
        let Some(i) = st.children().filter(|c| c.kind() == LOCAL_NAME).position(|n| n.text_range().contains_range(decl.range)) else { return false };
        let values: Vec<SyntaxNode> = st.children().find(|c| c.kind() == EXPR_LIST).map(|l| l.children().collect()).unwrap_or_default();
        match values.get(i) {
            Some(v) => v.kind() == TABLE_EXPR,
            None => !values.last().is_some_and(|v| matches!(v.kind(), CALL_EXPR | METHOD_CALL_EXPR | VARARG_EXPR)),
        }
    }

    /// Whether every use of the local (and of `self` in its methods) reads or
    /// writes a field, so no other code can reach the table. `t:m()` passes
    /// `t` on, so `m` must be a `function t:m()` of the file.
    fn contained(&self, d: usize, sites: &[Site]) -> bool {
        let selfs: Vec<usize> = (0..self.names.decls.len())
            .filter(|&s| self.names.decls[s].kind == DeclKind::SelfParam && self.method_owner(s, 0) == Some((Base::Decl(d), Vec::new())))
            .collect();
        // The `function t:m()` names.
        let methods: Vec<String> = selfs
            .iter()
            .filter_map(|&s| self.root.token_at_offset(self.names.decls[s].range.start()).right_biased())
            .filter_map(|colon| colon.parent())
            .filter_map(|fname| crate::util::tokens(&fname).filter(|t| t.kind() == NAME).last())
            .map(|t| t.text().to_string())
            .collect();
        let mine = |t: Target| matches!(t, Target::Decl(x) if x == d || selfs.contains(&x));
        if sites.iter().any(|s| s.dynamic && s.owner == (Base::Decl(d), Vec::new())) {
            return false;
        }
        self.names.refs.iter().filter(|r| mine(r.target)).all(|r| {
            let Some(tok) = self.root.token_at_offset(r.range.start()).right_biased() else { return false };
            let Some(parent) = tok.parent() else { return false };
            match parent.kind() {
                FUNC_NAME => crate::util::tokens(&parent).find(|t| t.kind() == NAME).is_some_and(|t| t == tok),
                NAME_EXPR => parent.parent().is_some_and(|p| match p.kind() {
                    INDEX_EXPR => p.first_child().is_some_and(|c| c == parent),
                    METHOD_CALL_EXPR => {
                        p.first_child().is_some_and(|c| c == parent) && crate::api::method_name(&p).is_some_and(|m| methods.iter().any(|x| x == m.text()))
                    }
                    // `t = { ... }` again; any other value brings its own fields.
                    VAR_LIST => p.children().position(|c| c == parent).is_some_and(|i| {
                        p.parent()
                            .and_then(|st| st.children().find(|c| c.kind() == EXPR_LIST))
                            .and_then(|l| l.children().nth(i))
                            .is_some_and(|v| v.kind() == TABLE_EXPR)
                    }),
                    _ => false,
                }),
                _ => false,
            }
        })
    }
}

/// `t.a` / `t['a']`: (field, key range, name range).
fn key(index: &SyntaxNode) -> Option<(String, TextRange, TextRange)> {
    if let Some(t) = crate::lint::field_name(index) {
        return Some((t.text().to_string(), t.text_range(), t.text_range()));
    }
    string_key(&index.children().nth(1)?)
}

/// `a = 1` / `['a'] = 1` in a constructor.
fn table_key(field: &SyntaxNode) -> Option<(String, TextRange, TextRange)> {
    match field.kind() {
        FIELD_NAMED => {
            let t = field.first_token().filter(|t| t.kind() == NAME)?;
            Some((t.text().to_string(), t.text_range(), t.text_range()))
        }
        FIELD_INDEXED => string_key(&field.first_child()?),
        _ => None,
    }
}

/// A literal string key: its text, the token's range and its content's.
pub(crate) fn string_key(e: &SyntaxNode) -> Option<(String, TextRange, TextRange)> {
    if e.kind() != LITERAL_EXPR {
        return None;
    }
    let t = e.first_token()?;
    let s = t.text();
    let (open, close) = match t.kind() {
        STRING => (1, s.get(..1)?.to_string()),
        // `[==[`: the brackets and the `=`s.
        LONG_STRING => {
            let level = s.get(1..)?.bytes().take_while(|&b| b == b'=').count();
            (2 + level, format!("]{}]", "=".repeat(level)))
        }
        _ => return None,
    };
    // Unterminated (error recovery) or too short: not a key.
    if s.len() < open + close.len() || !s.ends_with(close.as_str()) {
        return None;
    }
    let content = s.get(open..s.len() - close.len())?;
    // An escape names another string than its text.
    if t.kind() == STRING && content.contains('\\') {
        return None;
    }
    let start = t.text_range().start() + TextSize::from(open as u32);
    Some((content.to_string(), t.text_range(), TextRange::at(start, TextSize::of(content))))
}

fn fields<'a>(parse: &Parse, names: &'a Names) -> Fields<'a> {
    let decls = names.decls.iter().enumerate().filter(|(_, d)| d.kind == DeclKind::Local).map(|(i, d)| (d.range.start(), i)).collect();
    Fields { root: parse.syntax(), names, refs: names.refs.iter().map(|r| (r.range.start(), r.target)).collect(), decls }
}

/// The sites of the field whose key is at `offset`, as (key, writes); `None`
/// when no field key is there.
pub fn field_references(parse: &Parse, names: &Names, offset: TextSize) -> Option<Vec<(TextRange, bool)>> {
    let f = fields(parse, names);
    let sites = f.sites();
    let at = sites.iter().find(|s| !s.dynamic && s.key.contains_inclusive(offset))?;
    let mut out: Vec<(TextRange, bool)> = sites.iter().filter(|s| !s.dynamic && s.field == at.field && s.owner == at.owner).map(|s| (s.key, s.write)).collect();
    // `_G.x` is the global `x`.
    if let (Base::Global(g), []) = (&at.owner.0, at.owner.1.as_slice())
        && g == "_G"
    {
        let root = parse.syntax();
        out.extend(
            names
                .refs
                .iter()
                .filter(|r| r.target == Target::Global && root.text().slice(r.range) == at.field.as_str())
                .map(|r| (r.range, matches!(r.access, crate::names::Access::Write | crate::names::Access::ReadWrite))),
        );
    }
    Some(out)
}

/// References of the `function` keyword of a function a local function
/// returns: the keyword, the function, and every local of the file that
/// receives it from a call (`local f2 = f()`).
pub fn function_references(parse: &Parse, names: &Names, offset: TextSize) -> Option<Vec<TextRange>> {
    let root = parse.syntax();
    let kw = root.token_at_offset(offset).find(|t| t.kind() == FUNCTION_KW)?;
    let func = kw.parent().filter(|f| f.kind() == FUNCTION_EXPR)?;
    let list = func.parent().filter(|l| l.kind() == EXPR_LIST)?;
    list.parent().filter(|r| r.kind() == RETURN_STAT)?;
    let i = list.children().position(|c| c == func)?;
    let holder = list.ancestors().find(|a| a.kind() == FUNC_BODY)?.parent().filter(|h| h.kind() == LOCAL_FUNCTION_STAT)?;
    let name = holder.children_with_tokens().filter_map(|e| e.into_token()).find(|t| t.kind() == NAME)?;
    let d = names.decls.iter().position(|d| d.range == name.text_range())?;
    let mut out = vec![kw.text_range(), func.text_range()];
    for r in names.refs.iter().filter(|r| r.target == Target::Decl(d)) {
        let Some(call) = root.covering_element(r.range).ancestors().find(|a| a.kind() == NAME_EXPR).and_then(|e| e.parent()).filter(|c| c.kind() == CALL_EXPR) else {
            continue;
        };
        let Some(values) = call.parent().filter(|l| l.kind() == EXPR_LIST) else { continue };
        let Some(stat) = values.parent().filter(|s| s.kind() == LOCAL_STAT) else { continue };
        // The call spreads only as the last value.
        let Some(p) = values.children().position(|c| c == call).filter(|p| *p + 1 == values.children().count()) else { continue };
        if let Some(local) = stat.children().filter(|c| c.kind() == LOCAL_NAME).nth(p + i) {
            out.push(local.text_range());
        }
    }
    Some(out)
}

/// The ranges to replace to rename the field at `offset`; `None` when no field
/// key is there. Only a field of a local that never leaves the file's field
/// reads and writes: a global's or a returned table's fields are used
/// elsewhere.
pub fn field_rename(parse: &Parse, names: &Names, offset: TextSize, new_name: &str) -> Option<Result<Vec<TextRange>, String>> {
    let f = fields(parse, names);
    let sites = f.sites();
    let at = sites.iter().find(|s| !s.dynamic && s.key.contains_inclusive(offset))?;
    if !crate::is_name(new_name) {
        return Some(Err(format!("`{new_name}` is not a valid Lua name")));
    }
    let (Base::Decl(d), []) = (&at.owner.0, at.owner.1.as_slice()) else {
        return Some(Err("only a field of a local table can be renamed: other files may use this one".into()));
    };
    if !f.local_table(*d) {
        return Some(Err(format!("`{}` is not a table built in this file: its fields may be used elsewhere", f.names.decls[*d].name)));
    }
    if !f.contained(*d, &sites) {
        return Some(Err(format!("`{}` is passed, returned or indexed with a computed key: its fields may be used elsewhere", f.names.decls[*d].name)));
    }
    Some(Ok(sites.iter().filter(|s| !s.dynamic && s.field == at.field && s.owner == at.owner).map(|s| s.name).collect()))
}

/// The ranges to replace to rename what is at `offset`: a name, with the
/// `---@param` lines above its function when it is a parameter (from either
/// one), else a field.
pub fn rename(parse: &Parse, names: &Names, text: &str, offset: TextSize, new_name: &str) -> Result<Vec<TextRange>, String> {
    match names.rename(text, offset, new_name) {
        Ok(mut ranges) => {
            if let Some(crate::Symbol::Decl(d)) = names.symbol_at(offset)
                && names.decls[d].kind == DeclKind::Param
                && let Some(stat) = function_stat(parse, names.decls[d].range.start())
            {
                ranges.extend(doc_params(&stat, text).into_iter().filter(|(n, _)| *n == names.decls[d].name).map(|(_, r)| r));
            }
            Ok(ranges)
        }
        Err(e) => {
            // `---@param name` itself: the parameter it names.
            let comment = parse.syntax().token_at_offset(offset).find(|t| t.kind() == COMMENT);
            if let Some(stat) = comment.and_then(|c| c.parent())
                && let Some((name, _)) = doc_params(&stat, text).into_iter().find(|(_, r)| r.contains_inclusive(offset))
                && let Some(list) = stat_params(&stat)
                && let Some(d) = names.decls.iter().find(|d| d.kind == DeclKind::Param && d.name == name && list.text_range().contains_range(d.range))
            {
                return rename(parse, names, text, d.range.start(), new_name);
            }
            field_rename(parse, names, offset, new_name).unwrap_or(Err(e))
        }
    }
}

/// The statement whose own function has the parameter at `at` (not one
/// around a function passed as an argument).
fn function_stat(parse: &Parse, at: TextSize) -> Option<SyntaxNode> {
    let tok = parse.syntax().token_at_offset(at).right_biased()?;
    let list = tok.parent_ancestors().find(|a| a.kind() == PARAM_LIST)?;
    let holder = list.parent()?.parent()?;
    let stat = match holder.kind() {
        FUNCTION_STAT | LOCAL_FUNCTION_STAT => holder,
        FUNCTION_EXPR => holder.parent().filter(|l| l.kind() == EXPR_LIST)?.parent()?,
        _ => return None,
    };
    (stat_params(&stat)? == list).then_some(stat)
}

/// The parameter list of a statement's own function: `function f(...)`,
/// `local f = function(...)`, `return function(...)`.
fn stat_params(stat: &SyntaxNode) -> Option<SyntaxNode> {
    let body = match stat.kind() {
        FUNCTION_STAT | LOCAL_FUNCTION_STAT => stat.children().find(|c| c.kind() == FUNC_BODY)?,
        LOCAL_STAT | ASSIGN_STAT | RETURN_STAT => {
            let values = stat.children().find(|c| c.kind() == EXPR_LIST)?;
            values.children().find(|v| v.kind() == FUNCTION_EXPR)?.children().find(|c| c.kind() == FUNC_BODY)?
        }
        _ => return None,
    };
    body.children().find(|c| c.kind() == PARAM_LIST)
}

/// The `---@param name` lines heading a statement: (name, its range).
fn doc_params(stat: &SyntaxNode, text: &str) -> Vec<(String, TextRange)> {
    let mut out = Vec::new();
    for e in stat.children_with_tokens() {
        let Some(t) = e.into_token() else { break };
        match t.kind() {
            COMMENT => {
                let line = &text[t.text_range()];
                let Some(rest) = line.strip_prefix("---").map(str::trim_start).and_then(|r| r.strip_prefix("@param")) else { continue };
                if !rest.starts_with([' ', '\t']) {
                    continue;
                }
                let name = rest.trim_start();
                let start = line.len() - name.len();
                let len = name.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(name.len());
                if len > 0 {
                    let at = t.text_range().start() + TextSize::from(start as u32);
                    out.push((name[..len].to_string(), TextRange::at(at, TextSize::from(len as u32))));
                }
            }
            WHITESPACE => {}
            _ => break,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refs<'t>(text: &'t str, at: &str) -> Vec<&'t str> {
        let p = crate::parse(text);
        let n = crate::names(&p, text);
        let off = TextSize::from(text.find(at).unwrap() as u32);
        field_references(&p, &n, off).unwrap_or_default().into_iter().map(|(r, _)| &text[r]).collect()
    }

    fn rename(text: &str, at: &str, new: &str) -> Result<String, String> {
        let p = crate::parse(text);
        let n = crate::names(&p, text);
        let off = TextSize::from(text.find(at).unwrap() as u32);
        let mut ranges = field_rename(&p, &n, off, new).ok_or("no field")??;
        ranges.sort_by_key(|r| std::cmp::Reverse(r.start()));
        let mut out = text.to_string();
        for r in ranges {
            out.replace_range(std::ops::Range::<usize>::from(r), new);
        }
        Ok(out)
    }

    #[test]
    fn parameter_rename_with_its_doc() {
        let text = "---@param a number\n---@param ab string\nlocal function f(a, ab) return a, ab end\n";
        let p = crate::parse(text);
        let n = crate::names(&p, text);
        let off = TextSize::from(text.find("a, ab)").unwrap() as u32);
        let mut got: Vec<&str> = Vec::new();
        let mut ranges = super::rename(&p, &n, text, off, "x").unwrap();
        ranges.sort_by_key(|r| r.start());
        got.extend(ranges.iter().map(|r| &text[*r]));
        assert_eq!(got, ["a", "a", "a"]);
        assert_eq!(ranges[0], TextRange::at(TextSize::from(10), TextSize::from(1)));
        // From the doc line: the same edit.
        let mut from_doc = super::rename(&p, &n, text, TextSize::from(10), "x").unwrap();
        from_doc.sort_by_key(|r| r.start());
        assert_eq!(from_doc, ranges);
    }

    #[test]
    fn broken_string_keys() {
        // Unterminated, multibyte, a wrong long bracket, escaped: no key, no panic.
        for text in ["t['a\u{e9}", "t[\"\u{e9}", "t[ [=[x]] ]", "t['a\\'b'] = 1"] {
            let p = crate::parse(text);
            let n = crate::names(&p, text);
            for i in (0..=text.len()).filter(|&i| text.is_char_boundary(i)) {
                let found = field_references(&p, &n, TextSize::from(i as u32));
                assert!(found.is_none_or(|s| s.is_empty()), "{text:?} at {i}");
            }
        }
    }

    #[test]
    fn references_by_path() {
        let text = "local t = { a = 1, sub = { b = 2 } }\nt.a = t['a'] + t.sub.b\nlocal u = t\nprint(u.a)\nfunction t:m() return self.a, self:m() end\n";
        // `u.a` is another path; `self` in `t:m` is `t`.
        assert_eq!(refs(text, "a = 1"), ["a", "a", "'a'", "a"]);
        assert_eq!(refs(text, "b = 2"), ["b", "b"]);
        assert_eq!(refs(text, "m()"), ["m", "m"]);
    }

    #[test]
    fn rename_only_contained_locals() {
        let text = "local t = { a = 1 }\nt.a = t['a'] + 1\nfunction t:m() return self.a end\n";
        assert_eq!(rename(text, "a = 1", "b").unwrap(), "local t = { b = 1 }\nt.b = t['b'] + 1\nfunction t:m() return self.b end\n");
        assert!(rename(text, "a = 1", "1x").unwrap_err().contains("not a valid Lua name"));
        // A global's, a returned table's, a table indexed with a computed key.
        assert!(rename("g = { a = 1 }\nprint(g.a)\n", "a = 1", "b").unwrap_err().contains("local table"));
        assert!(rename("local M = { a = 1 }\nreturn M\n", "a = 1", "b").is_err());
        assert!(rename("local t = { a = 1 }\nprint(t[k])\n", "a = 1", "b").is_err());
        assert!(rename("local t = { a = 1 }\nfunction t:m() f(self) end\n", "a = 1", "b").is_err());
        assert!(rename("local t = { a = 1 }\nt = other\n", "a = 1", "b").is_err());
        // Not a table built here.
        assert!(rename("for _, v in pairs(list) do print(v.label) end\n", "label", "x").is_err());
        assert!(rename("local function f(p) return p.a end\n", "a end", "x").is_err());
        assert!(rename("local M = require 'm'\nM.foo()\n", "foo", "x").is_err());
        assert!(rename("local t = GetThing()\nprint(t.a)\n", "a)", "x").is_err());
        assert!(rename("local a, t = 1, f()\nprint(t.a)\n", "a)", "x").is_err());
        // `t:m()` hands `t` to a function not written as `t:m`.
        assert!(rename("local t = { a = 1 }\nfunction t.m(self) self.a = 2 end\nt:m()\n", "a = 1", "b").is_err());
        assert_eq!(
            rename("local t = { a = 1 }\nfunction t:m() return self.a end\nt:m()\n", "a = 1", "b").unwrap(),
            "local t = { b = 1 }\nfunction t:m() return self.b end\nt:m()\n"
        );
    }

    #[test]
    fn doc_param_of_the_own_function_only() {
        let text = "---@param data table\nfunction outer(data)\n  foo(function(data) return data end)\n  return data\nend\n";
        let p = crate::parse(text);
        let n = crate::names(&p, text);
        let doc = TextSize::from(10);
        // The inner `data`: its own function has no doc line.
        let inner = TextSize::from(text.find("function(data)").unwrap() as u32 + 9);
        let got = super::rename(&p, &n, text, inner, "x").unwrap();
        assert!(got.iter().all(|r| !r.contains(doc)), "{got:?}");
        // From the doc line: the outer one, with its two uses.
        let mut got = super::rename(&p, &n, text, doc, "x").unwrap();
        got.sort_by_key(|r| r.start());
        assert_eq!(got.len(), 3, "{got:?}");
        assert_eq!(got[0], TextRange::at(doc, TextSize::from(4)));
    }
}
