//! Name resolution inside one file: every local, parameter, loop variable and
//! label with the names that refer to it, and the global names. Lua scoping as
//! LuaLS (`parser/guide.lua` `getLocal`): a `local` statement's names are visible
//! after the statement, `local function` inside its own body, loop variables in
//! the loop body, a `repeat` body's locals in its `until` condition; labels in
//! their whole block within the same function.
//!
//! One preorder walk over the tree (rowan's walk is iterative, so chains nest
//! without limit).

use crate::{Parse, TextRange, TextSize};
use cfx_lua_syntax::SyntaxKind::{self, *};
use cfx_lua_syntax::{SyntaxNode, SyntaxToken};
use rowan::{NodeOrToken, WalkEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclKind {
    Local,
    LocalFunction,
    Param,
    ForVar,
    /// The implicit `self` of `function a:b()`; its range is the `:`.
    SelfParam,
    Label,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attrib {
    Const,
    Close,
}

/// What a local starts with, when the statement gives it a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Value {
    None,
    Table,
    Function,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decl {
    pub name: String,
    /// The name token (`:` for `self`).
    pub range: TextRange,
    pub kind: DeclKind,
    pub attrib: Option<Attrib>,
    pub value: Value,
    /// The statement or parameter list that declares it.
    pub owner: TextRange,
    /// The function expression a local starts with (`local f = function () end`).
    pub function: Option<TextRange>,
    /// For `self`: the object of `function obj:m()` when `obj` is a plain name.
    pub self_of: Option<Target>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Read,
    /// The object of a field assignment (`t.x = 1`, `function t.f()`): LuaLS does
    /// not count it as a use of a local holding a table constructor.
    FieldSet,
    Write,
    /// `x += 1`.
    ReadWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Decl(usize),
    Global,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameRef {
    pub range: TextRange,
    pub target: Target,
    pub access: Access,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vararg {
    /// The `...` parameter.
    pub range: TextRange,
    pub used: bool,
}

#[derive(Debug, Default, Clone)]
pub struct Names {
    pub decls: Vec<Decl>,
    /// In source order.
    pub refs: Vec<NameRef>,
    /// `(declaration, the visible local of the same name it hides)`.
    pub shadows: Vec<(usize, usize)>,
    pub varargs: Vec<Vararg>,
}

struct Scope {
    locals: Vec<(String, usize)>,
    /// Index into `Walk::labels`.
    id: usize,
    /// A function body: labels and the vararg stop here.
    function: bool,
}

struct Func {
    vararg: Option<usize>,
}

/// A name of a `local` statement waiting for the statement's end.
struct PendingLocal {
    name: String,
    range: TextRange,
    attrib: Option<Attrib>,
    value: Value,
    function: Option<TextRange>,
    owner: TextRange,
}

struct Walk<'a> {
    text: &'a str,
    names: Names,
    scopes: Vec<Scope>,
    funcs: Vec<Func>,
    /// Locals of the `local` statement being walked, visible once it ends.
    pending: Vec<Vec<PendingLocal>>,
    /// Loop variables, visible in the loop body.
    loop_vars: Vec<(String, TextRange, TextRange)>,
    /// The labels of every scope, by scope id; kept after the scope closes.
    labels: Vec<Vec<usize>>,
    /// `goto` names with the ids of the scopes up to the function's.
    gotos: Vec<(String, TextRange, Vec<usize>)>,
}

/// The names of a file.
pub fn names(parse: &Parse, text: &str) -> Names {
    let root = parse.syntax();
    let mut w = Walk {
        text,
        names: Names::default(),
        scopes: vec![Scope { locals: Vec::new(), id: 0, function: true }],
        funcs: vec![Func { vararg: None }],
        pending: Vec::new(),
        loop_vars: Vec::new(),
        labels: vec![Vec::new()],
        gotos: Vec::new(),
    };
    for event in root.preorder_with_tokens() {
        match event {
            WalkEvent::Enter(NodeOrToken::Node(n)) => w.enter(&n),
            WalkEvent::Leave(NodeOrToken::Node(n)) => w.leave(&n),
            WalkEvent::Enter(NodeOrToken::Token(t)) if t.kind() == NAME => w.name(&t),
            _ => {}
        }
    }
    w.resolve_gotos();
    w.names
}

fn name_text(text: &str, range: TextRange) -> &str {
    &text[range]
}

impl Walk<'_> {
    fn push(&mut self, function: bool) {
        let id = self.labels.len();
        self.labels.push(Vec::new());
        self.scopes.push(Scope { locals: Vec::new(), id, function });
    }

    fn lookup(&self, name: &str) -> Option<usize> {
        for scope in self.scopes.iter().rev() {
            if let Some(&(_, id)) = scope.locals.iter().rev().find(|(n, _)| n == name) {
                return Some(id);
            }
        }
        None
    }

    fn add_decl(&mut self, decl: Decl) -> usize {
        let id = self.names.decls.len();
        self.names.decls.push(decl);
        id
    }

    /// Declares a local-like name in the innermost scope.
    fn declare(&mut self, name: &str, range: TextRange, kind: DeclKind, attrib: Option<Attrib>, value: Value, owner: TextRange) -> usize {
        let hidden = if kind == DeclKind::SelfParam { None } else { self.lookup(name) };
        let id = self.add_decl(Decl { name: name.to_string(), range, kind, attrib, value, owner, function: None, self_of: None });
        if let Some(h) = hidden
            && name != "_"
        {
            self.names.shadows.push((id, h));
        }
        self.scopes.last_mut().unwrap().locals.push((name.to_string(), id));
        id
    }

    fn enter(&mut self, n: &SyntaxNode) {
        match n.kind() {
            BLOCK => {
                let parent = n.parent().map(|p| p.kind());
                match parent {
                    Some(REPEAT_STAT | FUNC_BODY) => {}
                    Some(NUMERIC_FOR_STAT | GENERIC_FOR_STAT) => {
                        self.push(false);
                        for (name, range, owner) in std::mem::take(&mut self.loop_vars) {
                            self.declare(&name, range, DeclKind::ForVar, None, Value::None, owner);
                        }
                    }
                    _ => self.push(false),
                }
            }
            REPEAT_STAT => self.push(false),
            FUNC_BODY => {
                self.push(true);
                self.funcs.push(Func { vararg: None });
                // `function a:b()` has an implicit `self`.
                if let Some(stat) = n.parent().filter(|p| p.kind() == FUNCTION_STAT)
                    && let Some(colon) = stat
                        .children()
                        .find(|c| c.kind() == FUNC_NAME)
                        .and_then(|f| f.children_with_tokens().filter_map(|e| e.into_token()).find(|t| t.kind() == COLON))
                {
                    let owner = n.text_range();
                    let names: Vec<SyntaxToken> = colon
                        .parent()
                        .into_iter()
                        .flat_map(|f| f.children_with_tokens().filter_map(|e| e.into_token()).filter(|t| t.kind() == NAME).collect::<Vec<_>>())
                        .collect();
                    let self_of = (names.len() == 2)
                        .then(|| self.lookup(name_text(self.text, names[0].text_range())).map_or(Target::Global, Target::Decl));
                    let id = self.declare("self", colon.text_range(), DeclKind::SelfParam, None, Value::None, owner);
                    self.names.decls[id].self_of = self_of;
                }
            }
            LOCAL_STAT => self.pending.push(Vec::new()),
            LOCAL_FUNCTION_STAT => {
                if let Some(name) = child_token(n, NAME) {
                    let range = name.text_range();
                    let owner = n.text_range();
                    self.declare(name_text(self.text, range), range, DeclKind::LocalFunction, None, Value::Function, owner);
                }
            }
            PARAM => {
                if let Some(dots) = child_token(n, DOT3) {
                    let id = self.names.varargs.len();
                    self.names.varargs.push(Vararg { range: dots.text_range(), used: false });
                    self.funcs.last_mut().unwrap().vararg = Some(id);
                }
            }
            VARARG_EXPR => {
                if let Some(id) = self.funcs.last().and_then(|f| f.vararg) {
                    self.names.varargs[id].used = true;
                }
            }
            _ => {}
        }
    }

    fn leave(&mut self, n: &SyntaxNode) {
        match n.kind() {
            BLOCK => {
                if !matches!(n.parent().map(|p| p.kind()), Some(REPEAT_STAT | FUNC_BODY)) {
                    self.scopes.pop();
                }
            }
            REPEAT_STAT => {
                self.scopes.pop();
            }
            FUNC_BODY => {
                self.scopes.pop();
                self.funcs.pop();
            }
            LOCAL_STAT => {
                let pending = self.pending.pop().unwrap_or_default();
                // Each name is checked against the scope before the statement:
                // `local a, a` hides nothing.
                let hidden: Vec<Option<usize>> = pending.iter().map(|p| self.lookup(&p.name)).collect();
                for (PendingLocal { name, range, attrib, value, function, owner }, hidden) in pending.into_iter().zip(hidden) {
                    let id = self.add_decl(Decl { name: name.clone(), range, kind: DeclKind::Local, attrib, value, owner, function, self_of: None });
                    if let Some(h) = hidden
                        && name != "_"
                    {
                        self.names.shadows.push((id, h));
                    }
                    self.scopes.last_mut().unwrap().locals.push((name, id));
                }
            }
            _ => {}
        }
    }

    fn name(&mut self, t: &SyntaxToken) {
        let Some(parent) = t.parent() else { return };
        let range = t.text_range();
        let text = name_text(self.text, range);
        match parent.kind() {
            LOCAL_NAME => {
                let attrib = parent.children().find(|c| c.kind() == ATTRIB).and_then(|a| {
                    let name = child_token(&a, NAME)?;
                    match &self.text[name.text_range()] {
                        "const" => Some(Attrib::Const),
                        "close" => Some(Attrib::Close),
                        _ => None,
                    }
                });
                let Some(stat) = parent.parent() else { return };
                let (value, function) = local_value(&stat, &parent);
                let owner = stat.text_range();
                if let Some(p) = self.pending.last_mut() {
                    p.push(PendingLocal { name: text.to_string(), range, attrib, value, function, owner });
                }
            }
            PARAM => {
                let owner = parent.parent().map_or(parent.text_range(), |p| p.text_range());
                self.declare(text, range, DeclKind::Param, None, Value::None, owner);
            }
            NUMERIC_FOR_STAT | GENERIC_FOR_STAT => {
                self.loop_vars.push((text.to_string(), range, parent.text_range()));
            }
            LABEL_STAT => {
                let owner = parent.text_range();
                let id = self.add_decl(Decl {
                    name: text.to_string(),
                    range,
                    kind: DeclKind::Label,
                    attrib: None,
                    value: Value::None,
                    owner,
                    function: None,
                    self_of: None,
                });
                let scope = self.scopes.last().unwrap().id;
                self.labels[scope].push(id);
            }
            GOTO_STAT => {
                // The label may come later in its block: resolved at the end.
                let mut chain = Vec::new();
                for scope in self.scopes.iter().rev() {
                    chain.push(scope.id);
                    if scope.function {
                        break;
                    }
                }
                self.gotos.push((text.to_string(), range, chain));
            }
            NAME_EXPR => {
                let access = name_access(&parent);
                let target = self.lookup(text).map_or(Target::Global, Target::Decl);
                self.names.refs.push(NameRef { range, target, access });
            }
            FUNC_NAME => {
                let first = parent.children_with_tokens().filter_map(|e| e.into_token()).find(|t| t.kind() == NAME);
                if first.as_ref() != Some(t) {
                    return;
                }
                let single = parent.children_with_tokens().filter_map(|e| e.into_token()).filter(|t| t.kind() == NAME).count() == 1;
                let access = if single { Access::Write } else { Access::FieldSet };
                let target = self.lookup(text).map_or(Target::Global, Target::Decl);
                self.names.refs.push(NameRef { range, target, access });
            }
            _ => {}
        }
    }
}

fn child_token(n: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxToken> {
    n.children_with_tokens().filter_map(|e| e.into_token()).find(|t| t.kind() == kind)
}

/// The value the `i`-th name of a `local` statement starts with.
/// with the function expression's range when it is one.
fn local_value(stat: &SyntaxNode, local_name: &SyntaxNode) -> (Value, Option<TextRange>) {
    if child_token(stat, EQ).is_none() {
        return (Value::None, None);
    }
    let Some(index) = stat.children().filter(|c| c.kind() == LOCAL_NAME).position(|c| &c == local_name) else {
        return (Value::None, None);
    };
    let Some(list) = stat.children().find(|c| c.kind() == EXPR_LIST) else { return (Value::None, None) };
    let expr = list.children().nth(index);
    match expr.as_ref().map(|e| e.kind()) {
        Some(TABLE_EXPR) => (Value::Table, None),
        Some(FUNCTION_EXPR) => (Value::Function, expr.as_ref().and_then(crate::util::trimmed)),
        Some(_) => (Value::Other, None),
        None => (Value::None, None),
    }
}

/// How a `NAME_EXPR` uses its name.
fn name_access(expr: &SyntaxNode) -> Access {
    let Some(parent) = expr.parent() else { return Access::Read };
    match parent.kind() {
        VAR_LIST => Access::Write,
        ASSIGN_STAT if child_token(&parent, IN_KW).is_some() => Access::Read,
        COMPOUND_ASSIGN_STAT if parent.first_child().as_ref() == Some(expr) => Access::ReadWrite,
        INDEX_EXPR if parent.first_child().as_ref() == Some(expr) => {
            let target = parent.parent().is_some_and(|g| g.kind() == VAR_LIST)
                || parent
                    .parent()
                    .is_some_and(|g| g.kind() == COMPOUND_ASSIGN_STAT && g.first_child().as_ref() == Some(&parent));
            if target { Access::FieldSet } else { Access::Read }
        }
        _ => Access::Read,
    }
}

impl Walk<'_> {
    /// Each `goto` refers to the label of its name in the nearest enclosing block
    /// of the same function; one with no visible label refers to nothing (a syntax
    /// error reports it).
    fn resolve_gotos(&mut self) {
        if self.gotos.is_empty() {
            return;
        }
        for (name, range, chain) in std::mem::take(&mut self.gotos) {
            let label = chain
                .iter()
                .find_map(|&s| self.labels[s].iter().copied().find(|&l| self.names.decls[l].name == name));
            if let Some(l) = label {
                self.names.refs.push(NameRef { range, target: Target::Decl(l), access: Access::Read });
            }
        }
        self.names.refs.sort_by_key(|r| r.range.start());
    }
}

impl Names {
    /// The declaration or global named by the token at `offset`.
    pub fn symbol_at(&self, offset: TextSize) -> Option<Symbol> {
        let at = |r: &TextRange| r.contains_inclusive(offset);
        // Refs are sorted and disjoint: the candidates are the last one starting at
        // or before `offset`, and the one before it when they touch (`a..b`).
        let i = self.refs.partition_point(|r| r.range.start() <= offset);
        if let Some(r) = self.refs[i.saturating_sub(2)..i].iter().rev().find(|r| at(&r.range)) {
            return Some(match r.target {
                Target::Decl(d) => Symbol::Decl(d),
                Target::Global => Symbol::Global(r.range),
            });
        }
        self.decls.iter().position(|d| d.kind != DeclKind::SelfParam && at(&d.range)).map(Symbol::Decl)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Symbol {
    Decl(usize),
    /// A global, by the range of one of its names.
    Global(TextRange),
}

impl Names {
    fn global_name(text: &str, r: TextRange) -> &str {
        &text[r]
    }

    /// Every reference to the symbol, with whether it writes.
    fn occurrences(&self, text: &str, sym: Symbol) -> Vec<(TextRange, bool)> {
        match sym {
            Symbol::Decl(d) => {
                let decl = &self.decls[d];
                let mut out = Vec::new();
                if decl.kind != DeclKind::SelfParam {
                    out.push((decl.range, true));
                }
                out.extend(
                    self.refs
                        .iter()
                        .filter(|r| r.target == Target::Decl(d))
                        .map(|r| (r.range, matches!(r.access, Access::Write | Access::ReadWrite))),
                );
                out
            }
            Symbol::Global(at) => {
                let name = Self::global_name(text, at);
                self.refs
                    .iter()
                    .filter(|r| r.target == Target::Global && Self::global_name(text, r.range) == name)
                    .map(|r| (r.range, matches!(r.access, Access::Write | Access::ReadWrite)))
                    .collect()
            }
        }
    }

    /// Where the name at `offset` is defined in this file: the declaration of a
    /// local or label, the assignments of a global.
    pub fn definition(&self, text: &str, offset: TextSize) -> Vec<TextRange> {
        match self.symbol_at(offset) {
            Some(Symbol::Decl(d)) if self.decls[d].kind == DeclKind::SelfParam => match self.decls[d].self_of {
                Some(Target::Decl(o)) => self.local_definition(o),
                Some(Target::Global) => {
                    // The object's name is the first name of the `function` statement,
                    // the first global reference after the `:`'s statement start.
                    let colon = self.decls[d].range;
                    let object = self.refs.iter().rev().find(|r| r.range.end() <= colon.start());
                    object.map_or_else(Vec::new, |r| self.definition(text, r.range.start()))
                }
                None => Vec::new(),
            },
            Some(Symbol::Decl(d)) => self.local_definition(d),
            Some(sym @ Symbol::Global(_)) => {
                self.occurrences(text, sym).into_iter().filter(|&(_, write)| write).map(|(r, _)| r).collect()
            }
            None => Vec::new(),
        }
    }

    /// A local's declaration, the function it starts with, and its assignments
    /// (LuaLS: `local` and `setlocal`).
    fn local_definition(&self, d: usize) -> Vec<TextRange> {
        let decl = &self.decls[d];
        let mut out = vec![decl.range];
        out.extend(decl.function);
        out.extend(
            self.refs
                .iter()
                .filter(|r| r.target == Target::Decl(d) && matches!(r.access, Access::Write | Access::ReadWrite))
                .map(|r| r.range),
        );
        out
    }

    /// Every occurrence of the name at `offset` in this file, the declaration first.
    pub fn references(&self, text: &str, offset: TextSize) -> Vec<TextRange> {
        self.symbol_at(offset).map(|s| self.occurrences(text, s).into_iter().map(|(r, _)| r).collect()).unwrap_or_default()
    }

    /// Like `references`, with whether each one writes.
    pub fn highlights(&self, text: &str, offset: TextSize) -> Vec<(TextRange, bool)> {
        self.symbol_at(offset).map(|s| self.occurrences(text, s)).unwrap_or_default()
    }

    /// The ranges to replace to rename the name at `offset`. `self` cannot be
    /// renamed; the new name must be a Lua name and not a keyword.
    pub fn rename(&self, text: &str, offset: TextSize, new_name: &str) -> Result<Vec<TextRange>, String> {
        if !is_name(new_name) {
            return Err(format!("`{new_name}` is not a valid Lua name"));
        }
        let sym = self.symbol_at(offset).ok_or_else(|| "no name here".to_string())?;
        if let Symbol::Decl(d) = sym
            && self.decls[d].kind == DeclKind::SelfParam
        {
            return Err("`self` cannot be renamed".into());
        }
        Ok(self.occurrences(text, sym).into_iter().map(|(r, _)| r).collect())
    }
}

/// A Lua name that is not a keyword (CfxLua has the Lua 5.4 keywords).
pub fn is_name(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else { return false };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !matches!(
            s,
            "and" | "break" | "do" | "else" | "elseif" | "end" | "false" | "for" | "function" | "goto" | "if" | "in"
                | "local" | "nil" | "not" | "or" | "repeat" | "return" | "then" | "true" | "until" | "while"
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    /// `(name text, target name or "global", access)` per reference.
    fn refs(text: &str) -> Vec<(String, String, Access)> {
        let n = names(&parse(text), text);
        n.refs
            .iter()
            .map(|r| {
                let target = match r.target {
                    Target::Decl(d) => format!("{}@{}", n.decls[d].name, u32::from(n.decls[d].range.start())),
                    Target::Global => "global".into(),
                };
                (text[r.range].to_string(), target, r.access)
            })
            .collect()
    }

    #[test]
    fn local_statement_names_are_visible_after_it() {
        let r = refs("local x = 1\nlocal x = x\nprint(x)");
        assert_eq!(r[0], ("x".into(), "x@6".into(), Access::Read));
        assert_eq!(r[2], ("x".into(), "x@18".into(), Access::Read));
    }

    #[test]
    fn local_function_sees_itself_and_loops_scope_their_variables() {
        let r = refs("local function f() f() end\nfor i = i, 2 do print(i) end");
        assert_eq!(r[0].1, "f@15");
        assert_eq!(r[1].1, "global"); // `i` in the bounds is the outer (global) one
        assert_eq!(r[3].1, "i@31");
    }

    #[test]
    fn repeat_condition_sees_the_body() {
        let r = refs("repeat local done = true until done");
        assert_eq!(r[0].1, "done@13");
    }

    #[test]
    fn cfx_in_unpacking_declares_locals() {
        let r = refs("local a, b in t\nprint(a, b)\na, b in t");
        assert_eq!(r[0], ("t".into(), "global".into(), Access::Read));
        assert_eq!(r[2].1, "a@6");
        assert_eq!(r[4], ("a".into(), "a@6".into(), Access::Write));
        assert_eq!(r[6], ("t".into(), "global".into(), Access::Read));
    }

    #[test]
    fn accesses() {
        let r = refs("local t = {}\nt.x = 1\nt.a.b = 2\nt += 1\nfunction t.f() end\nfunction g() end");
        let access: Vec<Access> = r.iter().map(|r| r.2).collect();
        assert_eq!(access, [Access::FieldSet, Access::Read, Access::ReadWrite, Access::FieldSet, Access::Write]);
    }

    #[test]
    fn method_self_and_varargs() {
        let text = "local mt = {}\nfunction mt:m(...) return self end\nlocal function v(...) end";
        let n = names(&parse(text), text);
        let me = n.decls.iter().find(|d| d.kind == DeclKind::SelfParam).unwrap();
        assert_eq!(me.self_of, Some(Target::Decl(0)));
        assert_eq!(n.definition(text, TextSize::from(text.find("self").unwrap() as u32)), [TextRange::new(6.into(), 8.into())]);
        assert_eq!(n.varargs.iter().map(|v| v.used).collect::<Vec<_>>(), [false, false]);
        let text = "local function v(...) return ... end";
        assert!(names(&parse(text), text).varargs[0].used);
    }

    #[test]
    fn gotos_stop_at_the_function() {
        let text = "::top::\nlocal function f() goto top end\ndo goto top end";
        let r = refs(text);
        let gotos: Vec<&(String, String, Access)> = r.iter().filter(|r| r.0 == "top").collect();
        assert_eq!(gotos.len(), 1, "{r:?}");
        assert_eq!(gotos[0].1, "top@2");
    }

    #[test]
    fn shadows_and_renames() {
        let text = "local x = 1\nlocal function f(x) end\nlocal _ = 1\nlocal _ = 2";
        let n = names(&parse(text), text);
        assert_eq!(n.shadows, [(2, 0)]);
        assert!(n.rename(text, 6.into(), "end").is_err());
        assert_eq!(n.rename(text, 6.into(), "y").unwrap(), [TextRange::new(6.into(), 7.into())]);
    }
}
