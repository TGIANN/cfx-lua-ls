//! The type of a name in one file, as LuaLS shows it (`integer`, `string|nil`,
//! `xPlayer`, `fun(a: string):boolean`): literals and operators, a local's
//! first value (or every value written when it starts empty), `---@type`,
//! `---@class`, `---@param`, `---@return` and `---@alias`, what a function of the
//! file returns, fields of table constructors and classes, `pairs` / `ipairs`
//! loops, and the library's `---@return` for globals the file does not define.
//! Other files are not read here: the workspace query does that.
//!
//! Every step is depth capped; what the cap or a missing rule leaves is
//! `unknown`.

use crate::luacats::{DocType as T, FunType, Key, Tag};
use crate::names::{Access, DeclKind, Names, Target};
use crate::{Parse, TextRange, TextSize};
use cfx_lua_syntax::SyntaxKind::*;
use cfx_lua_syntax::{SyntaxNode, SyntaxToken};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

const MAX_DEPTH: usize = 40;
const FLOW_BUDGET: usize = 20_000;

/// What an earlier statement does to a local, seen from a later read.
enum Effect {
    /// Sets it (or declares it): the walk stops here with this type.
    Set(T),
    /// The read only happens when it is not nil (`x ~= nil`).
    NonNil,
    /// The read only happens when it is truthy (`if x`, `not x or ...`): not
    /// nil, not `false`.
    Truthy,
    /// It may hold these values too (written inside a branch or loop).
    Also(Vec<T>),
    /// `if not x then x = V end`: not nil, or `V`.
    Fill(Vec<T>),
    /// `---@cast x +string, -?`.
    Cast(Vec<(char, T)>),
    /// `if type(x) == 'string' then`.
    Narrow(T),
    /// The read only happens when it is falsy (`if not x`, the `else` of
    /// `if x`): nil or `false`.
    Falsy,
    /// The read only happens when it is nil (`if x == nil`).
    Nil,
}

/// Without nil and `false` (`boolean` is `true`).
fn truthy(t: T) -> T {
    match non_nil(t) {
        T::Union(parts) => union(
            parts
                .into_iter()
                .filter(|p| !matches!(p, T::Bool(false)))
                .map(|p| if is(&p, "boolean") { T::Bool(true) } else { p })
                .collect(),
        ),
        t if is(&t, "boolean") => T::Bool(true),
        t => t,
    }
}

/// Only nil and `false` (`boolean` is `false`); `nil_only`: only nil.
fn falsy(t: T, nil_only: bool) -> T {
    if has_unknown(&t) || mentions_any(&t) {
        return t;
    }
    let kept: Vec<T> = members_of(&t)
        .into_iter()
        .filter_map(|m| match m {
            T::Name(n) if n == "nil" => Some(name("nil")),
            T::Name(n) if n == "boolean" && !nil_only => Some(T::Bool(false)),
            T::Bool(false) if !nil_only => Some(T::Bool(false)),
            _ => None,
        })
        .collect();
    union(kept)
}

/// The members of a type (`A` of `A`, `A` and nil of `A?`, the parts of `A|B`).
fn members_of(t: &T) -> Vec<T> {
    match t {
        T::Optional(i) => vec![(**i).clone(), name("nil")],
        T::Union(ps) => ps.iter().flat_map(members_of).collect(),
        t => vec![t.clone()],
    }
}

/// A union where `boolean` takes in `true` and `false`.
fn bool_union(parts: Vec<T>) -> T {
    let has_bool = parts.iter().flat_map(members_of).any(|m| is(&m, "boolean"));
    let parts = parts.iter().flat_map(members_of).filter(|m| !(has_bool && matches!(m, T::Bool(_)))).collect();
    union(parts)
}

fn non_nil(t: T) -> T {
    match t {
        T::Optional(inner) => *inner,
        T::Union(parts) => union(parts.into_iter().filter(|p| !is(p, "nil")).collect()),
        t => t,
    }
}

fn name(s: &str) -> T {
    T::Name(s.to_string())
}

fn unknown() -> T {
    name("unknown")
}

fn is(t: &T, n: &str) -> bool {
    matches!(t, T::Name(x) if x == n)
}

/// `a|b|c`, flattened and without repeats; one member stays itself. A `T?`
/// part counts as `T|nil`, and one type with nil from such a part shows as
/// `T?` again.
fn union(parts: Vec<T>) -> T {
    fn add(out: &mut Vec<T>, t: T) {
        if !out.contains(&t) {
            out.push(t);
        }
    }
    let mut out: Vec<T> = Vec::new();
    let mut optional = false;
    for p in parts {
        match p {
            T::Union(inner) => inner.into_iter().for_each(|i| add(&mut out, i)),
            T::Optional(inner) => {
                optional = true;
                add(&mut out, *inner);
                add(&mut out, name("nil"));
            }
            p => add(&mut out, p),
        }
    }
    if optional && out.len() == 2 && let Some(i) = out.iter().position(|t| is(t, "nil")) {
        out.remove(i);
        return T::Optional(Box::new(out.remove(0)));
    }
    match out.len() {
        0 => unknown(),
        1 => out.pop().unwrap_or_else(unknown),
        _ => T::Union(out),
    }
}

/// Integer, float or neither.
fn numeric(t: &T) -> Option<bool> {
    match t {
        T::Name(n) if n == "integer" => Some(true),
        T::Name(n) if n == "number" => Some(false),
        T::Int(_) => Some(true),
        _ => None,
    }
}

struct Infer<'a> {
    text: &'a str,
    names: &'a Names,
    root: SyntaxNode,
    classes: HashMap<String, crate::api::ClassDef>,
    aliases: HashMap<String, T>,
    depth: Cell<usize>,
    /// Statements the flow walk may still look at.
    flow_budget: Cell<usize>,
    /// The flow budget ran out: later answers may miss a narrowing.
    exhausted: Cell<bool>,
    /// `function a.b()` / `function a:b()` / `function f()` of the file by
    /// name (`a.b`, `f`), first one kept: its body.
    functions: std::cell::OnceCell<HashMap<String, SyntaxNode>>,
    /// Locals written after their declaration.
    written: std::cell::OnceCell<std::collections::HashSet<usize>>,
    /// Field writes with a `---@type` above them, by field name: (the object
    /// written to, the type).
    typed_fields: std::cell::OnceCell<HashMap<String, Vec<(SyntaxNode, T)>>>,
    /// The statement declaring each local asked.
    decl_stats: RefCell<HashMap<usize, Option<SyntaxNode>>>,
    /// Answers already worked out; `None` while one is being worked out, so a
    /// cycle (`local function f() return f() end`) ends as `unknown`.
    decls: RefCell<HashMap<usize, Option<T>>>,
    globals: RefCell<HashMap<String, Option<T>>>,
    bodies: RefCell<HashMap<TextRange, Option<Vec<T>>>>,
    /// The type of each expression node asked (its type depends on where it
    /// is, so the node is the key).
    exprs: RefCell<HashMap<(TextRange, cfx_lua_syntax::SyntaxKind), Option<T>>>,
}

/// The type of the name (or `function` keyword) at `offset`, LuaLS style.
pub fn type_at(parse: &Parse, names: &Names, text: &str, offset: TextSize) -> Option<String> {
    let root = parse.syntax();
    let tok = root.token_at_offset(offset).right_biased()?;
    let inf = Infer::new(root, names, text);
    let t = inf.token_type(&tok)?;
    Some(crate::luacats::view(&t))
}

/// The declared or inferred type of a local, for hover; `None` when nothing is
/// known.
pub fn decl_type(parse: &Parse, names: &Names, text: &str, decl: usize) -> Option<String> {
    let inf = Infer::new(parse.syntax(), names, text);
    let t = inf.decl_type(decl);
    (!is(&t, "unknown") && !is(&t, "any")).then(|| crate::luacats::view(&t))
}

/// A literal type as its base type (`'x'` as `string`, `1` as `integer`).
fn widen(t: &T) -> T {
    match t {
        T::Str(_) => name("string"),
        T::Int(_) => name("integer"),
        T::Bool(_) => name("boolean"),
        t => t.clone(),
    }
}

/// The fields a hover lists at most.
const HOVER_FIELDS: usize = 50;

/// One file's types, shared by many questions (inlay hints): the file's
/// classes and aliases are read once.
pub struct FileTypes<'a> {
    inf: Infer<'a>,
}

impl<'a> FileTypes<'a> {
    pub fn new(parse: &Parse, names: &'a Names, text: &'a str) -> FileTypes<'a> {
        FileTypes { inf: Infer::new(parse.syntax(), names, text) }
    }

    pub(crate) fn from_root(root: SyntaxNode, names: &'a Names, text: &'a str) -> FileTypes<'a> {
        FileTypes { inf: Infer::new(root, names, text) }
    }

    /// The expression is a string (`('%s'):`, a local holding one).
    pub(crate) fn is_string(&self, e: &SyntaxNode) -> bool {
        let t = non_nil(self.inf.expr_type(e));
        is(&t, "string") || matches!(t, T::Str(_))
    }

    /// The fields LuaLS's hover lists under a local: a table constructor's in
    /// source order (`a: integer = 1`, `[2]: string = 'b'`; true when it is
    /// one), else its type's: a class's `---@field`s with its parents' by name,
    /// a tuple's or a `{ x: T }` shape's members. None when there are none;
    /// past `HOVER_FIELDS` (LuaLS's `hover.previewFields` default) a last `...`.
    pub(crate) fn hover_fields(&self, d: usize) -> Option<(Vec<String>, bool)> {
        let inf = &self.inf;
        let stat = inf.decl_stat(d).filter(|s| s.kind() == LOCAL_STAT);
        let doc = stat.as_ref().is_some_and(|st| stat_tags(inf.text, st).iter().any(|t| matches!(t, Tag::Type { .. } | Tag::Class { .. })));
        if !doc
            && let Some(st) = &stat
            && let Some(index) = st.children().filter(|c| c.kind() == LOCAL_NAME).position(|n| n.text_range().contains_range(inf.names.decls[d].range))
            && let Some(table) = st.children().find(|c| c.kind() == EXPR_LIST).and_then(|l| l.children().nth(index)).filter(|v| v.kind() == TABLE_EXPR)
        {
            let fields = self.constructor_fields(&table);
            return (!fields.is_empty()).then_some((fields, true));
        }
        let mut fields = self.type_fields(&non_nil(inf.decl_type(d)));
        if fields.len() > HOVER_FIELDS {
            fields.truncate(HOVER_FIELDS);
            fields.push("...".to_string());
        }
        (!fields.is_empty()).then_some((fields, false))
    }

    fn constructor_fields(&self, table: &SyntaxNode) -> Vec<String> {
        let inf = &self.inf;
        let literal = |e: &SyntaxNode| -> Option<String> {
            let src: String = inf.text[e.text_range()].chars().filter(|c| !c.is_whitespace()).collect();
            let ok = match e.kind() {
                LITERAL_EXPR => src != "nil",
                UNARY_EXPR => src.starts_with('-') && src[1..].starts_with(|c: char| c.is_ascii_digit()),
                _ => false,
            };
            ok.then_some(src)
        };
        let mut out = Vec::new();
        let mut position = 0;
        for f in table.children() {
            let (key, value) = match f.kind() {
                FIELD_NAMED => {
                    let Some(name) = f.first_token().filter(|t| t.kind() == NAME) else { continue };
                    (name.text().to_string(), f.children().next())
                }
                FIELD_INDEXED => {
                    let parts: Vec<SyntaxNode> = f.children().collect();
                    let [k, v] = parts.as_slice() else { continue };
                    let Some(k) = literal(k) else { continue };
                    (format!("[{k}]"), Some(v.clone()))
                }
                FIELD_POS => {
                    position += 1;
                    (format!("[{position}]"), f.children().next())
                }
                _ => continue,
            };
            let Some(value) = value else { continue };
            if out.len() == HOVER_FIELDS {
                out.push("...".to_string());
                break;
            }
            let ty = crate::luacats::view(&inf.expr_type(&value));
            match literal(&value) {
                Some(l) => out.push(format!("{key}: {ty} = {l}")),
                None => out.push(format!("{key}: {ty}")),
            }
        }
        out
    }

    fn type_fields(&self, t: &T) -> Vec<String> {
        let member = |name: String, ty: &T| match ty {
            T::Optional(i) => format!("{name}?: {}", crate::luacats::view(i)),
            t => format!("{name}: {}", crate::luacats::view(t)),
        };
        match t {
            T::Name(class) if self.inf.classes.contains_key(class) => {
                let mut fields: Vec<(String, T)> = Vec::new();
                let mut stack = vec![class.clone()];
                let mut seen = Vec::new();
                while let Some(c) = stack.pop() {
                    if seen.contains(&c) || seen.len() > 16 {
                        continue;
                    }
                    let Some(def) = self.inf.classes.get(&c) else { continue };
                    for f in &def.fields {
                        if !fields.iter().any(|(n, _)| *n == f.name) {
                            fields.push((f.name.clone(), f.ty.clone()));
                        }
                    }
                    stack.extend(def.parents.iter().rev().cloned());
                    seen.push(c);
                }
                fields.sort_by(|a, b| a.0.cmp(&b.0));
                fields.into_iter().map(|(n, ty)| member(n, &ty)).collect()
            }
            T::Tuple(items) => items.iter().enumerate().map(|(i, ty)| member(format!("[{}]", i + 1), ty)).collect(),
            T::Table(shape) => shape
                .iter()
                .map(|f| {
                    let key = match &f.key {
                        Key::Name(n) => n.clone(),
                        Key::Index(k) => format!("[{}]", crate::luacats::view(k)),
                    };
                    let key = if f.optional { format!("{key}?") } else { key };
                    format!("{key}: {}", crate::luacats::view(&f.ty))
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// A function's signature as LuaLS's hover writes it: `(a: any, b?: T,
    /// ...any)`, its return lines (`  -> T`, `  2. name: T`) and whether it
    /// is `---@async`.
    pub(crate) fn signature(&self, body: &SyntaxNode) -> (String, Vec<String>, bool) {
        let inf = &self.inf;
        let stat = body.parent().and_then(|h| match h.kind() {
            FUNCTION_STAT | LOCAL_FUNCTION_STAT => Some(h),
            FUNCTION_EXPR => h.ancestors().find(|a| matches!(a.kind(), LOCAL_STAT | ASSIGN_STAT)),
            _ => None,
        });
        let tags = stat.map(|s| stat_tags(inf.text, &s)).unwrap_or_default();
        let mut params = Vec::new();
        for p in body.children().filter(|c| c.kind() == PARAM_LIST).flat_map(|l| l.children().filter(|p| p.kind() == PARAM).collect::<Vec<_>>()) {
            let Some(tok) = p.first_token() else { continue };
            if tok.kind() == DOT3 {
                let ty = tags
                    .iter()
                    .find_map(|t| match t {
                        Tag::Param { name, ty: Some(ty), .. } if name == "..." => Some(crate::luacats::view(ty)),
                        _ => None,
                    })
                    .unwrap_or_else(|| "any".to_string());
                params.push(format!("...{ty}"));
                continue;
            }
            params.push(match inf.param_type(&tok) {
                T::Optional(i) => format!("{}?: {}", tok.text(), crate::luacats::view(&i)),
                t => format!("{}: {}", tok.text(), crate::luacats::view(&t)),
            });
        }
        let doc: Vec<crate::luacats::FunRet> = tags
            .iter()
            .filter_map(|t| match t {
                Tag::Return { rets, .. } => Some(rets.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        let rets: Vec<(Option<String>, String)> = if doc.is_empty() {
            inf.body_returns(body).iter().map(|t| (None, crate::luacats::view(t))).collect()
        } else {
            doc.into_iter()
                .map(|r| {
                    let ty = if r.optional { T::Optional(Box::new(r.ty)) } else { r.ty };
                    (r.name, crate::luacats::view(&ty))
                })
                .collect()
        };
        let lines = rets
            .into_iter()
            .enumerate()
            .map(|(i, (n, ty))| {
                let head = if i == 0 { "  -> ".to_string() } else { format!("  {}. ", i + 1) };
                format!("{head}{}{ty}", n.map(|n| format!("{n}: ")).unwrap_or_default())
            })
            .collect();
        let is_async = tags.iter().any(|t| matches!(t, Tag::Async));
        (format!("({})", params.join(", ")), lines, is_async)
    }

    /// A table constructor where a `---@class` of this file is expected that
    /// leaves out required `---@field`s (not optional, not nil-able), parents'
    /// included: (the class, the missing names).
    pub(crate) fn missing_fields(&self, table: &SyntaxNode) -> Option<(String, Vec<String>)> {
        let inf = &self.inf;
        if inf.exhausted.get() {
            return None;
        }
        // `A|B`: the table must have what one of them needs.
        let classes: Vec<String> = match non_nil(inf.expected_type(table)?) {
            T::Name(c) => vec![c],
            T::Union(ps) => ps.into_iter().filter(|p| !is(p, "nil")).map(|p| if let T::Name(c) = p { Some(c) } else { None }).collect::<Option<Vec<String>>>()?,
            _ => return None,
        };
        let mut given: Vec<String> = Vec::new();
        let mut pos = 0;
        for field in table.children() {
            match field.kind() {
                FIELD_NAMED => given.extend(field.children_with_tokens().filter_map(|x| x.into_token()).find(|t| t.kind() == NAME).map(|t| t.text().to_string())),
                FIELD_POS => {
                    pos += 1;
                    given.push(format!("[{pos}]"));
                }
                FIELD_INDEXED => {
                    let key = field.children().next().filter(|k| k.kind() == LITERAL_EXPR).and_then(|k| k.first_token());
                    // A computed key may be any of them.
                    let k = key.as_ref()?;
                    match k.kind() {
                        NUMBER => given.push(format!("[{}]", k.text())),
                        _ => given.push(crate::util::unquote(k.text())?.to_string()),
                    }
                }
                _ => {}
            }
        }
        let mut best: Option<(String, Vec<String>)> = None;
        for class in classes {
            let missing = self.class_missing(&class, &given)?;
            if missing.is_empty() {
                return None;
            }
            if best.as_ref().is_none_or(|(_, m)| missing.len() < m.len()) {
                best = Some((class, missing));
            }
        }
        best
    }

    /// The required fields of a class of this file (parents included) not in
    /// `given`; None when the class or a parent is not all known here.
    fn class_missing(&self, class: &str, given: &[String]) -> Option<Vec<String>> {
        let inf = &self.inf;
        let class = class.to_string();
        inf.classes.get(&class)?;
        let mut required: Vec<String> = Vec::new();
        // `---@class (partial) B: A`: A's fields are not required.
        let partial = inf.text.lines().any(|l| {
            let t = l.trim_start().trim_start_matches('-').trim_start();
            t.starts_with("@class") && t.contains("partial") && t.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.')).any(|w| w == class)
        });
        let mut stack = vec![class.clone()];
        let mut seen = std::collections::HashSet::new();
        while let Some(c) = stack.pop() {
            if !seen.insert(c.clone()) || seen.len() > 16 {
                continue;
            }
            // A parent declared in another file may add fields: not told.
            let def = inf.classes.get(&c)?;
            for f in &def.fields {
                if !nil_able(&f.ty) && !required.contains(&f.name) {
                    required.push(f.name.clone());
                }
            }
            // `---@field [1] string`.
            for (k, v) in &def.indexes {
                if let T::Int(n) = k
                    && !nil_able(v)
                {
                    required.push(format!("[{n}]"));
                }
            }
            if !partial {
                stack.extend(def.parents.iter().filter(|p| !BASIC.contains(&p.as_str())).cloned());
            }
        }
        Some(required.into_iter().filter(|r| !given.contains(r)).collect())
    }

    /// Fields read (`v.f`, `v:m()`) on a value of a `---@class` of this file
    /// that neither the class (parents included) declares nor any code of the
    /// file writes under that name: (the name's range, the class, its chain).
    /// Silent for a class with `[K]` index fields, a parent outside the file
    /// or a basic parent (`table`): the caller still checks that no other file
    /// declares one of the chain.
    pub(crate) fn undefined_fields(&self, root: &SyntaxNode) -> Vec<(TextRange, Vec<String>)> {
        let inf = &self.inf;
        let mut out = Vec::new();
        let enums = self.enum_fields();
        if inf.classes.is_empty() && enums.is_empty() {
            return out;
        }
        // Every name the file writes as a field, anywhere, with what it is
        // written on: a write on a value of another class does not hide it.
        enum Writer {
            Unknown,
            Node(SyntaxNode),
            Token(SyntaxToken),
        }
        let mut written: HashMap<&str, Vec<Writer>> = HashMap::new();
        for n in root.descendants() {
            let name = match n.kind() {
                INDEX_EXPR if n.parent().is_some_and(|p| p.kind() == VAR_LIST) => {
                    crate::lint::field_name(&n).map(|f| (f, n.first_child().map_or(Writer::Unknown, Writer::Node)))
                }
                FIELD_NAMED => n.children_with_tokens().filter_map(|x| x.into_token()).find(|t| t.kind() == NAME).map(|f| (f, Writer::Unknown)),
                FUNC_NAME => {
                    let parts: Vec<SyntaxToken> = n.children_with_tokens().filter_map(|x| x.into_token()).filter(|t| t.kind() == NAME).collect();
                    let w = if parts.len() == 2 { Writer::Token(parts[0].clone()) } else { Writer::Unknown };
                    parts.last().cloned().map(|f| (f, w))
                }
                _ => None,
            };
            if let Some((t, w)) = name {
                written.entry(self.text_of(t.text_range())).or_default().push(w);
            }
            // A computed write (`t[k] = v`, `rawset`) may set any name.
            if n.kind() == INDEX_EXPR && n.parent().is_some_and(|p| p.kind() == VAR_LIST) && crate::lint::field_name(&n).is_none() {
                return out;
            }
        }
        let globals = self.global_classes();
        let mut chains: HashMap<String, Option<ClassChain>> = HashMap::new();
        // Whether a write of the name may be on a value of these classes.
        let hidden = |field: &str, names: &[String]| {
            written.get(field).is_some_and(|ws| {
                ws.iter().any(|w| {
                    let class = match w {
                        Writer::Unknown => None,
                        Writer::Node(o) => match non_nil(inf.expr_type(o)) {
                            T::Name(c) => Some(c),
                            _ => None,
                        },
                        Writer::Token(t) => self.token_class(t, &globals),
                    };
                    class.is_none_or(|c| names.contains(&c) || !inf.classes.contains_key(&c))
                })
            })
        };
        for n in root.descendants() {
            let (object, field) = match n.kind() {
                INDEX_EXPR if n.parent().is_none_or(|p| p.kind() != VAR_LIST) => match (n.first_child(), crate::lint::field_name(&n)) {
                    (Some(o), Some(f)) => (o, f),
                    _ => continue,
                },
                METHOD_CALL_EXPR => {
                    let Some(o) = n.first_child() else { continue };
                    let Some(f) = n.children_with_tokens().filter_map(|x| x.into_token()).find(|t| t.kind() == NAME) else { continue };
                    (o, f)
                }
                _ => continue,
            };
            if n.children_with_tokens().any(|c| c.kind() == QUESTION) {
                continue;
            }
            if inf.exhausted.get() {
                return out;
            }
            // `X.C` on an `---@enum X` without `C`.
            if let Some((name, keys)) = self.enum_of(&object, &enums) {
                if !keys.contains(field.text()) && !written.contains_key(field.text()) {
                    out.push((field.text_range(), vec![name]));
                }
                continue;
            }
            let T::Name(class) = non_nil(inf.expr_type(&object)) else { continue };
            // `---@generic T: A|B`: a field of neither.
            let classes = if inf.classes.contains_key(&class) { vec![class] } else { self.generic_bound_classes(&object, &class) };
            if classes.is_empty() {
                continue;
            }
            let mut names = Vec::new();
            let mut known = false;
            for class in &classes {
                let Some(chain) = chains.entry(class.clone()).or_insert_with(|| self.class_chain(class)).clone() else {
                    known = true;
                    break;
                };
                if chain.1.contains(field.text()) {
                    known = true;
                    break;
                }
                names.extend(chain.0);
            }
            if !known && !hidden(field.text(), &names) {
                out.push((field.text_range(), names));
            }
        }
        out
    }

    /// The `---@enum`s of the file whose table has only named fields: (the
    /// variable's declaration or global name, the enum, its keys).
    fn enum_fields(&self) -> Vec<(Result<usize, String>, String, std::collections::HashSet<String>)> {
        let inf = &self.inf;
        let mut out = Vec::new();
        for st in inf.root.descendants().filter(|n| matches!(n.kind(), LOCAL_STAT | ASSIGN_STAT)) {
            let Some(name) = stat_tags(inf.text, &st).into_iter().find_map(|t| match t {
                Tag::Enum { name, .. } => Some(name),
                _ => None,
            }) else {
                continue;
            };
            let Some(table) = st.children().find(|c| c.kind() == EXPR_LIST).and_then(|l| l.first_child()).filter(|t| t.kind() == TABLE_EXPR) else { continue };
            if table.children().any(|f| f.kind() != FIELD_NAMED) {
                continue;
            }
            let keys = table.children().filter_map(|f| f.children_with_tokens().filter_map(|x| x.into_token()).find(|t| t.kind() == NAME)).map(|t| t.text().to_string()).collect();
            let target = match st.kind() {
                LOCAL_STAT => {
                    let Some(n) = st.children().find(|c| c.kind() == LOCAL_NAME) else { continue };
                    let Some(d) = inf.decl_at(n.first_token().map_or(n.text_range(), |t| t.text_range())) else { continue };
                    Ok(d)
                }
                _ => {
                    let Some(v) = st.children().find(|c| c.kind() == VAR_LIST).and_then(|l| l.first_child()).filter(|v| v.kind() == NAME_EXPR) else { continue };
                    Err(inf.text[v.text_range()].to_string())
                }
            };
            out.push((target, name, keys));
        }
        out
    }

    /// The enum a name reads, with its keys.
    fn enum_of<'e>(&self, object: &SyntaxNode, enums: &'e [(Result<usize, String>, String, std::collections::HashSet<String>)]) -> Option<(String, &'e std::collections::HashSet<String>)> {
        let inf = &self.inf;
        if object.kind() != NAME_EXPR || enums.is_empty() {
            return None;
        }
        let target = inf.ref_at(object.first_token()?.text_range())?.target;
        enums
            .iter()
            .find(|(t, _, _)| match (t, target) {
                (Ok(d), Target::Decl(x)) => *d == x,
                (Err(g), Target::Global) => *g == inf.text[object.text_range()],
                _ => false,
            })
            .map(|(_, n, k)| (n.clone(), k))
    }

    /// The classes a `---@generic T: A|B` of the function around `e` is bound to.
    fn generic_bound_classes(&self, e: &SyntaxNode, g: &str) -> Vec<String> {
        let inf = &self.inf;
        let Some(st) = e.ancestors().find(|a| a.kind() == FUNC_BODY).and_then(|b| inf.doc_stat_of(&b)) else { return Vec::new() };
        let bound = stat_tags(inf.text, &st).into_iter().find_map(|t| match t {
            Tag::Generic { names } => names.into_iter().find(|(n, _)| n == g).and_then(|(_, b)| b),
            _ => None,
        });
        let members = match bound {
            Some(T::Name(c)) => vec![c],
            Some(T::Union(ps)) => ps.into_iter().filter_map(|p| if let T::Name(c) = p { Some(c) } else { None }).collect(),
            _ => return Vec::new(),
        };
        if members.iter().all(|c| inf.classes.contains_key(c)) { members } else { Vec::new() }
    }

    /// The class a name token's variable defines: `---@class C` above its
    /// `local` or global assignment, and `self` in `function v:m()` of such a
    /// variable.
    fn token_class(&self, tok: &SyntaxToken, globals: &HashMap<String, String>) -> Option<String> {
        let inf = &self.inf;
        match inf.ref_at(tok.text_range())?.target {
            Target::Global => globals.get(tok.text()).cloned(),
            Target::Decl(d) => {
                let decl = &inf.names.decls[d];
                match decl.kind {
                    DeclKind::Local => {
                        let st = inf.decl_stat(d)?;
                        // Only the first name of the statement takes the tag.
                        let first = st.children().find(|c| c.kind() == LOCAL_NAME)?;
                        if first.text_range() != decl.range && !first.text_range().contains_range(decl.range) {
                            return None;
                        }
                        class_tag(inf.text, &st)
                    }
                    DeclKind::SelfParam => {
                        let colon = inf.root.token_at_offset(decl.range.start()).right_biased()?;
                        let fname = colon.parent().filter(|p| p.kind() == FUNC_NAME)?;
                        let parts: Vec<SyntaxToken> = fname.children_with_tokens().filter_map(|e| e.into_token()).filter(|t| t.kind() == NAME).collect();
                        if parts.len() != 2 {
                            return None;
                        }
                        self.token_class(&parts[0], globals)
                    }
                    _ => None,
                }
            }
        }
    }

    /// Field names of each class's own table in this file: the constructor
    /// under `---@class C` and the writes on a variable of the class.
    fn class_table_fields(&self, globals: &HashMap<String, String>) -> HashMap<String, std::collections::HashSet<String>> {
        let inf = &self.inf;
        let mut out: HashMap<String, std::collections::HashSet<String>> = HashMap::new();
        for st in inf.root.descendants().filter(|s| matches!(s.kind(), LOCAL_STAT | ASSIGN_STAT)) {
            let Some(class) = class_tag(inf.text, &st) else { continue };
            let Some(table) = st.children().find(|c| c.kind() == EXPR_LIST).and_then(|l| l.first_child()).filter(|v| v.kind() == TABLE_EXPR) else { continue };
            let names = table.children().filter(|f| f.kind() == FIELD_NAMED).filter_map(|f| f.first_token().filter(|t| t.kind() == NAME));
            out.entry(class).or_default().extend(names.map(|t| t.text().to_string()));
        }
        for (o, f, _, _) in self.field_writes() {
            if let Some(class) = self.token_class(&o, globals) {
                out.entry(class).or_default().insert(f.text().to_string());
            }
        }
        out
    }

    /// Globals a `---@class` tag sits on (`---@class C` above `C = {}`).
    fn global_classes(&self) -> HashMap<String, String> {
        let inf = &self.inf;
        let mut out = HashMap::new();
        for st in inf.root.descendants().filter(|s| s.kind() == ASSIGN_STAT) {
            let Some(target) = st.children().find(|c| c.kind() == VAR_LIST).and_then(|v| v.first_child()).filter(|t| t.kind() == NAME_EXPR) else { continue };
            let Some(tok) = target.first_token() else { continue };
            if inf.ref_at(tok.text_range()).is_some_and(|r| r.target == Target::Global)
                && let Some(c) = class_tag(inf.text, &st)
            {
                out.insert(tok.text().to_string(), c);
            }
        }
        out
    }

    /// The writes of the file as (object token, field token, a `:` method, the
    /// object expression of `o.f = v`): `o.f = v` and `function o.f()` /
    /// `function o:f()` on a plain name.
    fn field_writes(&self) -> Vec<(SyntaxToken, SyntaxToken, bool, Option<SyntaxNode>)> {
        let mut out = Vec::new();
        for n in self.inf.root.descendants() {
            match n.kind() {
                INDEX_EXPR if n.parent().is_some_and(|p| p.kind() == VAR_LIST) => {
                    let (Some(o), Some(f)) = (n.first_child().filter(|o| o.kind() == NAME_EXPR), crate::lint::field_name(&n)) else { continue };
                    if let Some(t) = o.first_token() {
                        out.push((t, f, false, Some(o)));
                    }
                }
                FUNC_NAME => {
                    let parts: Vec<SyntaxToken> = n.children_with_tokens().filter_map(|e| e.into_token()).filter(|t| t.kind() == NAME).collect();
                    if parts.len() == 2 {
                        let colon = n.children_with_tokens().any(|e| e.kind() == COLON);
                        out.push((parts[0].clone(), parts[1].clone(), colon, None));
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// The table constructor a class-defining statement assigns first.
    fn class_constructor(st: &SyntaxNode) -> Option<SyntaxNode> {
        st.children().find(|c| c.kind() == EXPR_LIST)?.first_child().filter(|v| v.kind() == TABLE_EXPR)
    }

    /// inject-field: a field written on a value of a `---@class` of this file
    /// (or a `---@type { a: T }` local) that the type does not declare and no
    /// code of the file defines on the class's own variable; with `(exact)`
    /// also writes on that variable and its constructor's fields. (The
    /// field's range, its name, the class chain, the message.) As with
    /// undefined-field, the caller checks no other file declares the chain.
    pub(crate) fn inject_fields(&self) -> Vec<(TextRange, Vec<String>, String)> {
        let inf = &self.inf;
        let mut out = Vec::new();
        let tables = inf.text.contains("---@type {") || inf.text.contains("---@type{");
        if inf.classes.is_empty() && !tables {
            return out;
        }
        let globals = self.global_classes();
        let writes = self.field_writes();
        // Names defined on each class's own variable.
        let mut defined: HashMap<String, std::collections::HashSet<String>> = HashMap::new();
        for (o, f, _, _) in &writes {
            if let Some(c) = self.token_class(o, &globals) {
                defined.entry(c).or_default().insert(f.text().to_string());
            }
        }
        let mut exact_tables = Vec::new();
        for st in inf.root.descendants().filter(|s| matches!(s.kind(), LOCAL_STAT | ASSIGN_STAT)) {
            let Some(c) = class_tag(inf.text, &st) else { continue };
            let Some(table) = Self::class_constructor(&st) else { continue };
            let names: Vec<SyntaxToken> = table
                .children()
                .filter(|f| f.kind() == FIELD_NAMED)
                .filter_map(|f| f.children_with_tokens().filter_map(|x| x.into_token()).find(|t| t.kind() == NAME))
                .collect();
            if inf.classes.get(&c).is_some_and(|d| d.exact) {
                exact_tables.push((c.clone(), names.clone()));
            }
            defined.entry(c).or_default().extend(names.iter().map(|t| t.text().to_string()));
        }
        let message = |class: &str, field: &str, fix: String| format!("Fields cannot be injected into the reference of `{class}` for `{field}`. {fix}").trim_end().to_string();
        for (c, names) in exact_tables {
            let Some((chain, fields)) = self.class_chain(&c) else { continue };
            for t in names.iter().filter(|t| !fields.contains(t.text())) {
                out.push((t.text_range(), chain.clone(), message(&c, t.text(), String::new())));
            }
        }
        for (o, f, method, object) in &writes {
            if inf.exhausted.get() {
                return out;
            }
            let own = self.token_class(o, &globals);
            if let Some(c) = &own {
                let self_node = inf.ref_at(o.text_range()).is_some_and(|r| matches!(r.target, Target::Decl(d) if inf.names.decls[d].kind == DeclKind::SelfParam));
                if !inf.classes.get(c).is_some_and(|d| d.exact) || (*method && !self_node) {
                    continue;
                }
            }
            // The object's type where it is written (LuaLS's view there: `x = x or {}`
            // makes it `C|table`, which takes any field).
            let t = match (object, inf.ref_at(o.text_range()).map(|r| r.target)) {
                (Some(e), Some(Target::Decl(_))) => inf.expr_type(e),
                (None, Some(Target::Decl(d))) => inf.decl_type(d),
                _ => continue,
            };
            let t = match non_nil(t) {
                T::Union(parts) => {
                    let parts: Vec<T> = parts.into_iter().filter(|p| !is(p, "nil") && !matches!(p, T::Bool(false)) && !is(p, "false")).collect();
                    if parts.len() != 1 {
                        continue;
                    }
                    parts.into_iter().next().unwrap_or_else(unknown)
                }
                t => t,
            };
            match t {
                T::Name(class) => {
                    let Some((chain, fields)) = self.class_chain(&class) else { continue };
                    if fields.contains(f.text()) {
                        continue;
                    }
                    // A reference: what the class's own variable defines counts too.
                    if own.is_none() && chain.iter().any(|c| defined.get(c).is_some_and(|d| d.contains(f.text()))) {
                        continue;
                    }
                    let fix = if own.is_none() { format!("To do so, use `---@class` for `{}`.", o.text()) } else { String::new() };
                    out.push((f.text_range(), chain, message(&class, f.text(), fix)));
                }
                T::Table(shape) if own.is_none() => {
                    // Only a shape a doc comment gives: a constructor's shape takes any field.
                    let Some(Target::Decl(d)) = inf.ref_at(o.text_range()).map(|r| r.target) else { continue };
                    let doc = inf.decl_stat(d).is_some_and(|st| stat_tags(inf.text, &st).iter().any(|t| matches!(t, Tag::Type { tys } if matches!(tys.first(), Some(T::Table(_))))));
                    if !doc || shape.iter().any(|s| !matches!(s.key, Key::Name(_))) || shape.iter().any(|s| matches!(&s.key, Key::Name(n) if n == f.text())) {
                        continue;
                    }
                    let view = crate::luacats::view(&T::Table(shape));
                    out.push((f.text_range(), Vec::new(), message(&view, f.text(), "To allow injection, add `[any]: any` to the definition.".to_string())));
                }
                _ => {}
            }
        }
        out
    }

    /// invisible: a `---@field private` / `protected` field of a class of
    /// this file read or written outside that class (its own variable, `self`
    /// or a function stored on it), or for `protected` outside its
    /// subclasses: (the field's range, the class declaring it, the message).
    /// `private` / `protected`: `doc.privateName` / `doc.protectedName` globs
    /// for the fields without a visibility of their own, `---@field`s or
    /// fields of the class's own table (its constructor, `C.f = v`,
    /// `function C:f()`, `self.f = v` in its methods).
    /// The caller checks no other file declares that class.
    /// `lua`: the names are Lua patterns found anywhere in the field name
    /// (`Lua.doc.regengine` `lua`), else whole-name globs.
    pub(crate) fn invisible_fields(&self, private: &[String], protected: &[String], lua: bool) -> Vec<(TextRange, String, String)> {
        let inf = &self.inf;
        let mut out = Vec::new();
        let named = !private.is_empty() || !protected.is_empty();
        if !named
            && !inf.classes.values().any(|c| c.fields.iter().any(|f| matches!(f.visibility, Some(crate::luacats::Visibility::Private | crate::luacats::Visibility::Protected))))
        {
            return out;
        }
        let globals = self.global_classes();
        let table_fields = if named { self.class_table_fields(&globals) } else { HashMap::new() };
        for n in inf.root.descendants() {
            let (object, field) = match n.kind() {
                INDEX_EXPR => match (n.first_child(), crate::lint::field_name(&n)) {
                    (Some(o), Some(f)) => (o, f),
                    _ => continue,
                },
                METHOD_CALL_EXPR => match (n.first_child(), crate::api::method_name(&n)) {
                    (Some(o), Some(f)) => (o, f),
                    _ => continue,
                },
                _ => continue,
            };
            if inf.exhausted.get() {
                return out;
            }
            let T::Name(class) = non_nil(inf.expr_type(&object)) else { continue };
            // The class in the chain declaring the field.
            let owner = self.field_owner(&class, field.text()).or_else(|| {
                let (chain, _) = self.class_chain(&class)?;
                chain.into_iter().find(|c| table_fields.get(c).is_some_and(|f| f.contains(field.text()))).map(|c| (c, None))
            });
            let Some((owner, vis)) = owner else { continue };
            use crate::luacats::Visibility as V;
            // `doc.privateName` before `doc.protectedName`, as LuaLS reads them.
            let by_name = |names: &[String]| names.iter().any(|g| if lua { crate::luapat::find(field.text(), g) } else { crate::manifest::glob_exact(g, field.text()) });
            let vis = vis.or_else(|| {
                if by_name(private) {
                    Some(V::Private)
                } else if by_name(protected) {
                    Some(V::Protected)
                } else {
                    None
                }
            });
            let Some(vis) = vis else { continue };
            if !matches!(vis, V::Private | V::Protected) {
                continue;
            }
            // Whose code this is: the object's own class, else the class of the
            // variable the enclosing function is stored on.
            let mine = object
                .first_token()
                .filter(|_| object.kind() == NAME_EXPR)
                .and_then(|t| self.token_class(&t, &globals))
                .or_else(|| self.env_class(&n, &globals));
            let visible = match &mine {
                None => false,
                Some(m) if *m == owner => true,
                Some(m) => vis == V::Protected && self.class_chain(m).is_some_and(|(chain, _)| chain.contains(&owner)),
            };
            if !visible {
                let message = match vis {
                    V::Private => format!("Field `{}` is private, it can only be accessed in class `{owner}`.", field.text()),
                    _ => format!("Field `{}` is protected, it can only be accessed in class `{owner}` and its subclasses.", field.text()),
                };
                out.push((field.text_range(), owner, message));
            }
        }
        out
    }

    /// await-in-sync and not-yieldable, over `---@async` functions of the file
    /// and the library's (`coroutine.yield`): (range, code, message).
    pub(crate) fn async_calls(&self) -> Vec<(TextRange, &'static str, String)> {
        let inf = &self.inf;
        let mut out = Vec::new();
        if !inf.text.contains("async") && !inf.text.contains("coroutine.yield") {
            return out;
        }
        for call in inf.root.descendants().filter(|c| c.kind() == CALL_EXPR) {
            let Some(callee) = call.first_child() else { continue };
            let args: Vec<SyntaxNode> = call.children().find(|c| c.kind() == ARG_LIST).map(|a| a.children().collect()).unwrap_or_default();
            let body = self.callee_body(&callee);
            // The main chunk counts as async.
            let sync = call.ancestors().find(|a| a.kind() == FUNC_BODY).and_then(|b| b.parent()).is_some_and(|h| !self.function_async(&h));
            let linked = |i: usize| body.as_ref().is_some_and(|b| self.param_called(b, i));
            if sync && (self.value_async(&callee) || args.iter().enumerate().any(|(i, a)| self.value_async(a) && linked(i))) {
                out.push((callee.text_range(), "await-in-sync", "Async function can only be called in async function.".to_string()));
            }
            for (i, a) in args.iter().enumerate() {
                if self.value_async(a) && !linked(i) && !self.yieldable(&callee, body.as_ref(), i) {
                    out.push((
                        a.text_range(),
                        "not-yieldable",
                        format!(
                            "The {}th parameter of this function was not marked as yieldable, but an async function was passed in. (Use `---@param name async fun()` to mark as yieldable)",
                            i + 1
                        ),
                    ));
                }
            }
        }
        out
    }

    /// The function body a callee names: a local function, a `local f =
    /// function`, `function f()` / `function M.f()` of the file.
    fn callee_body(&self, callee: &SyntaxNode) -> Option<SyntaxNode> {
        let inf = &self.inf;
        inf.function_body(callee).or_else(|| {
            if callee.kind() != INDEX_EXPR {
                return None;
            }
            let key: String = inf.text[callee.text_range()].split_whitespace().collect();
            inf.file_functions().get(&key).cloned()
        })
    }

    /// `---@async` on a function: above its statement, or on its header's line
    /// (`function () ---@async`).
    fn function_async(&self, holder: &SyntaxNode) -> bool {
        let inf = &self.inf;
        let stat = match holder.kind() {
            FUNCTION_STAT | LOCAL_FUNCTION_STAT => Some(holder.clone()),
            FUNCTION_EXPR => holder.ancestors().find(|a| matches!(a.kind(), LOCAL_STAT | ASSIGN_STAT)).filter(|s| {
                s.children().find(|c| c.kind() == EXPR_LIST).and_then(|l| l.first_child()).is_some_and(|f| f == *holder)
            }),
            _ => None,
        };
        if stat.is_some_and(|s| stat_tags(inf.text, &s).iter().any(|t| matches!(t, Tag::Async))) {
            return true;
        }
        let Some(body) = holder.children().find(|c| c.kind() == FUNC_BODY) else { return false };
        let Some(close) = body.children().find(|c| c.kind() == PARAM_LIST).map(|p| p.text_range().end()) else { return false };
        let rest = &inf.text[usize::from(close)..];
        rest[..rest.find('\n').unwrap_or(rest.len())].contains("---@async")
    }

    /// A value that is an async function: a function expression, a name of
    /// one, a local or parameter typed `async fun()`, a library function.
    fn value_async(&self, e: &SyntaxNode) -> bool {
        let inf = &self.inf;
        match e.kind() {
            FUNCTION_EXPR => self.function_async(e),
            PAREN_EXPR => e.first_child().is_some_and(|c| self.value_async(&c)),
            NAME_EXPR | INDEX_EXPR => {
                if let Some(h) = self.callee_body(e).and_then(|b| b.parent()) {
                    return self.function_async(&h);
                }
                if e.kind() == NAME_EXPR
                    && let Some(Target::Decl(d)) = e.first_token().and_then(|t| inf.ref_at(t.text_range())).map(|r| r.target)
                {
                    return matches!(non_nil(inf.decl_type(d)), T::Fun(f) if f.is_async);
                }
                let path: String = inf.text[e.text_range()].split_whitespace().collect();
                let lib = crate::library::library();
                lib.get(&path).iter().any(|item| lib.doc(item).contains("@async"))
            }
            _ => false,
        }
    }

    /// Whether the `i`th parameter of a function is called in it (or handed
    /// to `pcall` / `xpcall`).
    fn param_called(&self, body: &SyntaxNode, i: usize) -> bool {
        let inf = &self.inf;
        let Some(param) = body.children().find(|c| c.kind() == PARAM_LIST).and_then(|l| l.children().filter(|p| p.kind() == PARAM).nth(i)) else { return false };
        let Some(tok) = param.first_token().filter(|t| t.kind() == NAME) else { return false };
        let Some(d) = inf.decl_at(tok.text_range()) else { return false };
        inf.names.refs.iter().filter(|r| r.target == Target::Decl(d)).any(|r| {
            let Some(e) = inf.root.token_at_offset(r.range.start()).right_biased().and_then(|t| t.parent()).filter(|e| e.kind() == NAME_EXPR) else { return false };
            if e.ancestors().find(|a| a.kind() == FUNC_BODY).as_ref() != Some(body) {
                return false;
            }
            let Some(p) = e.parent() else { return false };
            match p.kind() {
                CALL_EXPR => p.first_child().as_ref() == Some(&e),
                ARG_LIST => {
                    p.first_child().as_ref() == Some(&e)
                        && p.parent().and_then(|c| c.first_child()).is_some_and(|f| matches!(&inf.text[f.text_range()], "pcall" | "xpcall"))
                }
                _ => false,
            }
        })
    }

    /// Whether the `i`th parameter of the called function takes an async
    /// function: typed `any` or `async fun()`, untyped, or `...`; a callee
    /// with no known definition takes anything.
    fn yieldable(&self, callee: &SyntaxNode, body: Option<&SyntaxNode>, i: usize) -> bool {
        let inf = &self.inf;
        if let Some(body) = body {
            let params: Vec<SyntaxNode> = body.children().find(|c| c.kind() == PARAM_LIST).map(|l| l.children().filter(|p| p.kind() == PARAM).collect()).unwrap_or_default();
            let Some(param) = params.get(i).or_else(|| params.last().filter(|p| p.first_token().is_some_and(|t| t.kind() == DOT3))) else { return true };
            let Some(tok) = param.first_token() else { return true };
            if tok.kind() == DOT3 {
                return true;
            }
            return match inf.param_type(&tok) {
                T::Fun(f) => f.is_async,
                t => is(&t, "any") || is(&t, "unknown"),
            };
        }
        let path: String = inf.text[callee.text_range()].split_whitespace().collect();
        let lib = crate::library::library();
        let Some(item) = lib.get(&path).first() else { return true };
        match lib.params(item).get(i) {
            None => true,
            Some((name, ty)) => name == "..." || ty.as_deref().is_none_or(|t| t == "any" || t.starts_with("async")),
        }
    }

    /// The class of the chain from `class` that declares `field`, with the
    /// field's visibility.
    fn field_owner(&self, class: &str, field: &str) -> Option<(String, Option<crate::luacats::Visibility>)> {
        let mut stack = vec![class.to_string()];
        let mut seen = std::collections::HashSet::new();
        while let Some(c) = stack.pop() {
            if !seen.insert(c.clone()) || seen.len() > 16 {
                continue;
            }
            let def = self.inf.classes.get(&c)?;
            if let Some(f) = def.fields.iter().find(|f| f.name == field) {
                return Some((c, f.visibility));
            }
            stack.extend(def.parents.iter().cloned());
        }
        None
    }

    /// The class whose variable stores the function around `n`
    /// (`function C:m()`, `function C.m()`, `C.m = function()`).
    fn env_class(&self, n: &SyntaxNode, globals: &HashMap<String, String>) -> Option<String> {
        let body = n.ancestors().find(|a| a.kind() == FUNC_BODY)?;
        let holder = body.parent()?;
        match holder.kind() {
            FUNCTION_STAT => {
                let fname = holder.children().find(|c| c.kind() == FUNC_NAME)?;
                let parts: Vec<SyntaxToken> = fname.children_with_tokens().filter_map(|e| e.into_token()).filter(|t| t.kind() == NAME).collect();
                (parts.len() == 2).then(|| self.token_class(&parts[0], globals)).flatten()
            }
            FUNCTION_EXPR => {
                let list = holder.parent().filter(|p| p.kind() == EXPR_LIST)?;
                let stat = list.parent().filter(|p| p.kind() == ASSIGN_STAT)?;
                let i = list.children().position(|c| c == holder)?;
                let target = stat.children().find(|c| c.kind() == VAR_LIST)?.children().nth(i)?;
                let object = target.first_child().filter(|o| target.kind() == INDEX_EXPR && o.kind() == NAME_EXPR)?;
                self.token_class(&object.first_token()?, globals)
            }
            _ => None,
        }
    }

    /// A class of this file with its parents (all of this file) and their
    /// `---@field` names; None when the chain leaves the file, extends a basic
    /// type or has `[K]` index fields.
    fn class_chain(&self, class: &str) -> Option<ClassChain> {
        let inf = &self.inf;
        let mut names = Vec::new();
        let mut fields = std::collections::HashSet::new();
        let mut stack = vec![class.to_string()];
        while let Some(c) = stack.pop() {
            if names.contains(&c) {
                continue;
            }
            if names.len() > 16 || BASIC.contains(&c.as_str()) {
                return None;
            }
            let def = inf.classes.get(&c)?;
            if !def.indexes.is_empty() {
                return None;
            }
            fields.extend(def.fields.iter().map(|f| f.name.clone()));
            stack.extend(def.parents.iter().cloned());
            names.push(c);
        }
        Some((names, fields))
    }

    fn text_of(&self, r: TextRange) -> &'a str {
        &self.inf.text[r]
    }

    /// A local used as an object (`x.f`, `x:m()`, `x()`) or written key
    /// (`t[x] = v`) that may still be nil there, after the checks around it.
    /// Only a local whose doc comment says it may be nil: `---@type T?` (or
    /// `T|nil`), `---@param x? T`.
    pub(crate) fn need_check_nil(&self, e: &SyntaxNode) -> bool {
        let inf = &self.inf;
        if e.kind() != NAME_EXPR || inf.exhausted.get() {
            return false;
        }
        let Some(parent) = e.parent() else { return false };
        // `x?.f`, `x?:m()`, `x?.()`: checked by the operator.
        let safe = parent.children_with_tokens().any(|c| c.kind() == QUESTION);
        let used = match parent.kind() {
            INDEX_EXPR | METHOD_CALL_EXPR | CALL_EXPR if parent.first_child().as_ref() == Some(e) => !safe,
            INDEX_EXPR => parent.parent().is_some_and(|p| p.kind() == VAR_LIST),
            _ => false,
        };
        if !used {
            return false;
        }
        let Some(Target::Decl(d)) = e.first_token().and_then(|t| inf.ref_at(t.text_range())).map(|r| r.target) else { return false };
        // An upvalue: the code between its writes and this function's calls is not followed.
        let decl = &inf.names.decls[d];
        let inner = e.ancestors().find(|a| a.kind() == FUNC_BODY).map(|b| b.text_range());
        if inner.is_some_and(|b| !b.contains_range(decl.range)) {
            return false;
        }
        let declared = match decl.kind {
            DeclKind::Local => inf.decl_stat(d).is_some_and(|st| stat_tags(inf.text, &st).iter().any(|t| matches!(t, Tag::Type { .. }))),
            DeclKind::Param => true,
            _ => false,
        };
        if !declared || !maybe_nil(&inf.decl_type(d)) {
            return false;
        }
        let t = inf.expr_type(e);
        !inf.exhausted.get() && maybe_nil(&t) && !has_unknown(&t) && !mentions_any(&t)
    }

    /// A value that surely does not fit a declared type: (its type, the
    /// declared one).
    pub(crate) fn value_mismatch(&self, e: &SyntaxNode, want: &T) -> Option<(String, String)> {
        let inf = &self.inf;
        if inf.exhausted.get() {
            return None;
        }
        let mut got = inf.expr_type(e);
        // `x = 3` under `---@type 1|2`: the literal, not `integer`.
        if let Some(lit) = literal_type(e).filter(|_| inf.wants_literal(want, 0)) {
            got = lit;
        }
        if inf.exhausted.get() {
            return None;
        }
        // `1.5` where only `integer` fits (LuaLS: a float is not an integer).
        if fractional_literal(e) && float_misfits(want) {
            return Some(("number".to_string(), crate::luacats::view(want)));
        }
        if inf.cast(&got, want, 0) != Some(false) {
            return None;
        }
        Some((crate::luacats::view(&got), crate::luacats::view(want)))
    }

    /// The writes of an assignment that surely do not fit: (target, code,
    /// message). A local keeps the type it was declared with (`cast-local-type`);
    /// a field of a `---@class` of this file takes its `---@field` type
    /// (`assign-type-mismatch`).
    pub(crate) fn assign_mismatches(&self, stat: &SyntaxNode) -> Vec<(TextRange, &'static str, String)> {
        let inf = &self.inf;
        let mut out = Vec::new();
        let vars: Vec<SyntaxNode> = stat.children().find(|c| c.kind() == VAR_LIST).map(|v| v.children().collect()).unwrap_or_default();
        let values: Vec<SyntaxNode> = stat.children().find(|c| c.kind() == EXPR_LIST).map(|v| v.children().collect()).unwrap_or_default();
        let stat_types: Vec<T> = stat_tags(inf.text, stat)
            .into_iter()
            .find_map(|t| match t {
                Tag::Type { tys } => Some(tys),
                _ => None,
            })
            .unwrap_or_default();
        for (i, var) in vars.iter().enumerate() {
            // `---@type string` above `x = f()`: the comment says what is written.
            if let Some(got) = stat_types.get(i).filter(|t| !has_unknown(t) && !mentions_any(t))
                && var.kind() == NAME_EXPR
                && let Some(Target::Decl(d)) = var.first_token().and_then(|t| inf.ref_at(t.text_range())).map(|r| r.target)
                && let Some(want) = self.declared_local(d)
            {
                // A declared `number` may be fractional: it does not fit `integer`.
                let float = is(got, "number") && float_misfits(&want);
                if (float || inf.cast(got, &want, 0) == Some(false)) && !inf.exhausted.get() {
                    let (got, want) = (crate::luacats::view(got), crate::luacats::view(&want));
                    out.push((var.text_range(), "cast-local-type", format!("This variable is defined as type `{want}`. Cannot convert its type to `{got}`.")));
                }
                continue;
            }
            // A call last spreads; only plain one-to-one values are checked, and
            // not names or `a or b` (the flow inference may miss a narrowing).
            let Some(value) = values.get(i).filter(|v| !(i + 1 == values.len() && vars.len() > values.len() && matches!(v.kind(), CALL_EXPR | METHOD_CALL_EXPR))) else { continue };
            if !matches!(value.kind(), LITERAL_EXPR | TABLE_EXPR | FUNCTION_EXPR | CALL_EXPR | METHOD_CALL_EXPR | UNARY_EXPR) && !inf.fixed_doc_local(value) {
                continue;
            }
            if inf.exhausted.get() {
                return out;
            }
            match var.kind() {
                NAME_EXPR => {
                    let Some(Target::Decl(d)) = var.first_token().and_then(|t| inf.ref_at(t.text_range())).map(|r| r.target) else { continue };
                    // Only a local whose type a doc comment declares: one typed by its
                    // first value is reused freely (`local found = false`, then
                    // `found = item`), and one declared empty takes every value.
                    let decl = &inf.names.decls[d];
                    if decl.kind == DeclKind::Local {
                        let tagged = inf.decl_stat(d).is_some_and(|st| {
                            stat_tags(inf.text, &st).iter().any(|t| matches!(t, Tag::Type { .. } | Tag::Class { .. }))
                        });
                        if !tagged {
                            continue;
                        }
                    } else if decl.kind != DeclKind::Param {
                        continue;
                    }
                    let want = inf.decl_type(d);
                    if is(&want, "nil") || has_unknown(&want) {
                        continue;
                    }
                    if let Some((got, want)) = self.value_mismatch(value, &want) {
                        out.push((var.text_range(), "cast-local-type", format!("This variable is defined as type `{want}`. Cannot convert its type to `{got}`.")));
                    }
                }
                INDEX_EXPR => {
                    let (Some(object), Some(field)) = (var.first_child(), crate::lint::field_name(var)) else { continue };
                    let T::Name(class) = non_nil(inf.expr_type(&object)) else { continue };
                    let declared = inf.classes.get(&class).and_then(|c| c.fields.iter().find(|f| f.name == field.text())).map(|f| f.ty.clone());
                    let Some(want) = declared.or_else(|| self.typed_field(&class, field.text())).or_else(|| self.written_field(&class, field.text(), &object)) else { continue };
                    if has_unknown(&want) || mentions_any(&want) {
                        continue;
                    }
                    if let Some((got, want)) = self.value_mismatch(value, &want) {
                        out.push((var.text_range(), "assign-type-mismatch", format!("Cannot assign `{got}` to `{want}`.")));
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// The values of `---@type T` `local x = v` that surely do not fit:
    /// (the name, its type, the declared one). The values checked are those
    /// of `assign_mismatches`.
    pub(crate) fn local_mismatches(&self, stat: &SyntaxNode) -> Vec<(TextRange, String, String)> {
        let inf = &self.inf;
        let mut out = Vec::new();
        let tags = stat_tags(inf.text, stat);
        // `---@class B` above `local b = a`: `b` is a `B`.
        let class = tags.iter().find_map(|t| match t {
            Tag::Class { name, generics, .. } if generics.is_empty() => Some(name.clone()),
            _ => None,
        });
        let Some(tys) = tags.into_iter().find_map(|t| match t {
            Tag::Type { tys } => Some(tys),
            _ => None,
        }) else {
            let Some(class) = class else { return out };
            let (Some(target), Some(value)) = (stat.children().find(|c| c.kind() == LOCAL_NAME), stat.children().find(|c| c.kind() == EXPR_LIST).and_then(|l| l.first_child())) else {
                return out;
            };
            if !inf.fixed_doc_local(&value) {
                return out;
            }
            let (want, got) = (name(&class), inf.expr_type(&value));
            // Only classes unrelated either way (`---@class Child: Parent` on a parent's value is left alone).
            if !inf.exhausted.get() && inf.cast(&got, &want, 0) == Some(false) && inf.cast(&want, &got, 0) == Some(false) {
                out.push((target.text_range(), crate::luacats::view(&got), crate::luacats::view(&want)));
            }
            return out;
        };
        let names: Vec<SyntaxNode> = stat.children().filter(|c| c.kind() == LOCAL_NAME).collect();
        let values: Vec<SyntaxNode> = stat.children().find(|c| c.kind() == EXPR_LIST).map(|v| v.children().collect()).unwrap_or_default();
        for ((name, value), want) in names.iter().zip(&values).zip(&tys) {
            if !matches!(value.kind(), LITERAL_EXPR | FUNCTION_EXPR | CALL_EXPR | METHOD_CALL_EXPR | UNARY_EXPR) && !inf.fixed_doc_local(value) {
                continue;
            }
            if is(want, "nil") || has_unknown(want) || mentions_any(want) {
                continue;
            }
            if let Some((got, want)) = self.value_mismatch(value, want) {
                out.push((name.text_range(), got, want));
            }
        }
        out
    }

    /// The type a doc comment declares for a local (`---@type`, `---@class`)
    /// or a parameter (`---@param`); None for a local typed by its value.
    fn declared_local(&self, d: usize) -> Option<T> {
        let inf = &self.inf;
        let decl = &inf.names.decls[d];
        let tagged = match decl.kind {
            DeclKind::Local => inf.decl_stat(d).is_some_and(|st| stat_tags(inf.text, &st).iter().any(|t| matches!(t, Tag::Type { .. } | Tag::Class { .. }))),
            DeclKind::Param => true,
            _ => false,
        };
        let want = inf.decl_type(d);
        (tagged && !is(&want, "nil") && !has_unknown(&want) && !mentions_any(&want)).then_some(want)
    }

    /// The type of a field the class's own variable is given in this file
    /// (`---@class A local m = { x = 1 }`, `m.x = 1`): the union of the
    /// values, when each is a literal, constructor or function. A write on
    /// the variable itself is checked against its constructor alone.
    fn written_field(&self, class: &str, field: &str, object: &SyntaxNode) -> Option<T> {
        let inf = &self.inf;
        let globals = self.global_classes();
        let own = |o: &SyntaxNode| o.kind() == NAME_EXPR && o.first_token().is_some_and(|t| self.token_class(&t, &globals).as_deref() == Some(class));
        let constructor_only = own(object);
        let mut found: Vec<T> = Vec::new();
        let mut take = |v: &SyntaxNode| -> bool {
            match v.kind() {
                LITERAL_EXPR if v.first_token().is_some_and(|t| t.kind() == NIL_KW) => true,
                LITERAL_EXPR | TABLE_EXPR | FUNCTION_EXPR => {
                    found.push(inf.expr_type(v));
                    true
                }
                _ => false,
            }
        };
        for st in inf.root.descendants().filter(|s| matches!(s.kind(), LOCAL_STAT | ASSIGN_STAT)) {
            let values: Vec<SyntaxNode> = st.children().find(|c| c.kind() == EXPR_LIST).map(|l| l.children().collect()).unwrap_or_default();
            // The constructor under `---@class C`.
            if class_tag(inf.text, &st).as_deref() == Some(class)
                && let Some(table) = values.first().filter(|v| v.kind() == TABLE_EXPR)
            {
                for f in table.children().filter(|f| f.kind() == FIELD_NAMED) {
                    let is_field = f.children_with_tokens().filter_map(|x| x.into_token()).find(|t| t.kind() == NAME).is_some_and(|t| t.text() == field);
                    if is_field && let Some(v) = f.first_child() && !take(&v) {
                        return None;
                    }
                }
            }
            if st.kind() != ASSIGN_STAT || constructor_only {
                continue;
            }
            let vars: Vec<SyntaxNode> = st.children().find(|c| c.kind() == VAR_LIST).map(|l| l.children().collect()).unwrap_or_default();
            for (i, var) in vars.iter().enumerate() {
                if var.kind() != INDEX_EXPR || crate::lint::field_name(var).is_none_or(|f| f.text() != field) || !var.first_child().is_some_and(|o| own(&o)) {
                    continue;
                }
                match values.get(i) {
                    Some(v) if take(v) => {}
                    _ => return None,
                }
            }
        }
        if found.is_empty() || found.iter().any(|t| has_unknown(t) || mentions_any(t)) {
            return None;
        }
        Some(union(found.iter().map(widen).collect()))
    }

    /// The type a `---@type` comment gives a field of a class of this file where
    /// it is written (`---@type integer` above `m.x = 1`, `m` a value of `class`).
    fn typed_field(&self, class: &str, field: &str) -> Option<T> {
        let inf = &self.inf;
        if !inf.classes.contains_key(class) {
            return None;
        }
        let typed = inf.typed_fields.get_or_init(|| {
            let mut out: HashMap<String, Vec<(SyntaxNode, T)>> = HashMap::new();
            for stat in inf.root.descendants().filter(|n| n.kind() == ASSIGN_STAT) {
                // Only statements that start with a comment carry a `---@type`.
                if stat.first_token().is_none_or(|t| t.kind() != COMMENT) {
                    continue;
                }
                let Some(var) = stat.children().find(|c| c.kind() == VAR_LIST).and_then(|v| v.first_child()) else { continue };
                let (Some(f), Some(object)) = (crate::lint::field_name(&var).filter(|_| var.kind() == INDEX_EXPR), var.first_child()) else { continue };
                let ty = stat_tags(inf.text, &stat).into_iter().find_map(|t| match t {
                    Tag::Type { tys } => tys.into_iter().next(),
                    _ => None,
                });
                if let Some(ty) = ty {
                    out.entry(f.text().to_string()).or_default().push((object, ty));
                }
            }
            out
        });
        typed.get(field)?.iter().find_map(|(object, ty)| (non_nil(inf.expr_type(object)) == T::Name(class.to_string())).then(|| ty.clone()))
    }

    /// The entries of a table constructor whose value surely does not fit the
    /// type the constructor is declared with (`---@type T[]`, a `---@field`,
    /// `{ x: T, [1]: U }`): (where, its type, the declared one). `{ v }` is
    /// `[1] = v`; the values checked are those of `assign_mismatches`.
    pub(crate) fn field_mismatches(&self, table: &SyntaxNode) -> Vec<(TextRange, String, String)> {
        let inf = &self.inf;
        let mut out = Vec::new();
        if inf.exhausted.get() {
            return out;
        }
        let Some(outer) = inf.expected_type(table) else { return out };
        if has_unknown(&outer) || mentions_any(&outer) {
            return out;
        }
        let mut pos = 0i64;
        for field in table.children() {
            let tokens: Vec<SyntaxToken> = field.children_with_tokens().filter_map(|x| x.into_token()).collect();
            let (key, value, at) = match field.kind() {
                FIELD_POS => {
                    pos += 1;
                    let Some(v) = field.first_child() else { continue };
                    let at = v.text_range();
                    (Key::Index(T::Int(pos)), v, at)
                }
                FIELD_NAMED => {
                    let (Some(k), Some(v)) = (tokens.iter().find(|t| t.kind() == NAME), field.first_child()) else { continue };
                    (Key::Name(k.text().to_string()), v, k.text_range())
                }
                FIELD_INDEXED => {
                    let nodes: Vec<SyntaxNode> = field.children().collect();
                    let (Some(k), Some(v)) = (nodes.first(), nodes.get(1)) else { continue };
                    let (Some(open), Some(close)) = (tokens.iter().find(|t| t.kind() == L_BRACK), tokens.iter().find(|t| t.kind() == R_BRACK)) else { continue };
                    let lit = k.first_token().filter(|_| k.kind() == LITERAL_EXPR);
                    let key = match lit.as_ref().map(|t| (t.kind(), t.text())) {
                        Some((NUMBER, n)) => match n.parse::<i64>() {
                            Ok(i) => Key::Index(T::Int(i)),
                            Err(_) => continue,
                        },
                        Some((STRING, s)) => match crate::util::unquote(s) {
                            Some(s) if crate::names::is_name(s) => Key::Name(s.to_string()),
                            _ => continue,
                        },
                        Some(_) => continue,
                        None => Key::Index(inf.expr_type(k)),
                    };
                    (key, v.clone(), TextRange::new(open.text_range().start(), close.text_range().end()))
                }
                _ => continue,
            };
            if !matches!(value.kind(), LITERAL_EXPR | FUNCTION_EXPR | CALL_EXPR | METHOD_CALL_EXPR | UNARY_EXPR) && !inf.fixed_doc_local(&value) {
                continue;
            }
            let Some(want) = inf.member_of_type(&outer, &key) else { continue };
            if is(&want, "nil") || has_unknown(&want) || mentions_any(&want) {
                continue;
            }
            if let Some((got, want)) = self.value_mismatch(&value, &want) {
                out.push((at, got, want));
            }
        }
        out
    }

    /// The `---@return` lines of the function of this file a plain call
    /// reaches; None without them, or with `---@overload` / `---@generic`.
    pub(crate) fn call_doc_returns(&self, call: &SyntaxNode) -> Option<Vec<crate::luacats::FunRet>> {
        let inf = &self.inf;
        if call.kind() != CALL_EXPR {
            return None;
        }
        let body = inf.function_body(&call.first_child()?)?;
        let tags = stat_tags(inf.text, &inf.doc_stat_of(&body)?);
        if tags.iter().any(|t| matches!(t, Tag::Overload(..) | Tag::Generic { .. })) {
            return None;
        }
        let rets: Vec<crate::luacats::FunRet> = tags
            .into_iter()
            .filter_map(|t| match t {
                Tag::Return { rets, .. } => Some(rets),
                _ => None,
            })
            .flatten()
            .collect();
        (!rets.is_empty()).then_some(rets)
    }

    /// Whether a declared type surely does not fit another: (both shown).
    pub(crate) fn type_mismatch(&self, got: &T, want: &T) -> Option<(String, String)> {
        let inf = &self.inf;
        if has_unknown(got) || has_unknown(want) || mentions_any(want) || inf.cast(got, want, 0) != Some(false) || inf.exhausted.get() {
            return None;
        }
        Some((crate::luacats::view(got), crate::luacats::view(want)))
    }

    /// An argument of a call to a function of this file whose parameter type
    /// it surely does not fit: (the argument's type, the parameter's).
    pub(crate) fn param_mismatch(&self, arg: &SyntaxNode) -> Option<(String, String)> {
        let inf = &self.inf;
        if inf.exhausted.get() {
            return None;
        }
        let call = arg.parent()?.parent()?;
        let callee = call.first_child()?;
        let local = match call.kind() {
            // A local function, or `function M.f()` / `function F()` of this file.
            CALL_EXPR => {
                inf.function_body(&callee).is_some() || {
                    let want: String = inf.text[callee.text_range()].split_whitespace().collect();
                    inf.file_functions().contains_key(&want)
                }
            }
            METHOD_CALL_EXPR => {
                callee.kind() == NAME_EXPR
                    && crate::api::method_name(&call).is_some_and(|m| inf.file_functions().contains_key(&format!("{}.{}", &inf.text[callee.text_range()], m.text())))
            }
            _ => false,
        };
        if !local {
            return self.class_call_mismatch(&call, &callee, arg);
        }
        let mut want = inf.expected_type(arg)?;
        // `---@generic T: string|table` and `---@param x T`: the bound.
        if let T::Name(g) = &want {
            let body = match call.kind() {
                CALL_EXPR => inf.function_body(&callee),
                _ => crate::api::method_name(&call).and_then(|m| inf.method_body(&callee, m.text())),
            };
            let bound = body.and_then(|b| inf.doc_stat_of(&b)).and_then(|st| {
                stat_tags(inf.text, &st).into_iter().find_map(|t| match t {
                    Tag::Generic { names } => names.into_iter().find(|(n, _)| n == g).map(|(_, b)| b),
                    _ => None,
                })
            });
            match bound {
                Some(Some(b)) => want = b,
                Some(None) => return None,
                None => {}
            }
        }
        // A table constructor is none of an enum's table values.
        if arg.kind() == TABLE_EXPR
            && let T::Name(n) = &want
            && matches!(inf.aliases.get(n), Some(T::Generic(g, _)) if g == OPAQUE_ENUM)
        {
            return Some(("table".to_string(), n.clone()));
        }
        let mut got = inf.expr_type(arg);
        if let Some(lit) = literal_type(arg).filter(|_| inf.wants_literal(&want, 0)) {
            got = lit;
        }
        if inf.exhausted.get() || inf.cast(&got, &want, 0) != Some(false) {
            return None;
        }
        Some((crate::luacats::view(&got), crate::luacats::view(&want)))
    }

    /// `C(x)` on a local `---@class C` with `---@overload fun(x: T): C`: an
    /// argument no overload takes.
    fn class_call_mismatch(&self, call: &SyntaxNode, callee: &SyntaxNode, arg: &SyntaxNode) -> Option<(String, String)> {
        let inf = &self.inf;
        if call.kind() != CALL_EXPR || callee.kind() != NAME_EXPR {
            return None;
        }
        let Target::Decl(d) = inf.ref_at(callee.first_token()?.text_range())?.target else { return None };
        let tags = stat_tags(inf.text, &inf.decl_stat(d)?);
        if !tags.iter().any(|t| matches!(t, Tag::Class { .. })) {
            return None;
        }
        let i = arg.parent()?.children().position(|c| c == *arg)?;
        let wants: Vec<T> = tags
            .into_iter()
            .filter_map(|t| match t {
                Tag::Overload(T::Fun(f)) => Some(f.params.get(i).map(|p| p.ty.clone())),
                _ => None,
            })
            .collect::<Option<Vec<Option<T>>>>()?
            .into_iter()
            .collect::<Option<Vec<T>>>()?;
        let got = inf.expr_type(arg);
        let first = wants.first()?.clone();
        for w in &wants {
            if has_unknown(w) || mentions_any(w) || inf.cast(&got, w, 0) != Some(false) {
                return None;
            }
        }
        (!inf.exhausted.get()).then(|| (crate::luacats::view(&got), crate::luacats::view(&first)))
    }

    /// `C:f()` on `function C.f(a)` of this file: the object is passed as `a`,
    /// and does not fit its `---@param`: (the `:`, its type, the parameter's).
    pub(crate) fn self_mismatch(&self, call: &SyntaxNode) -> Option<(TextRange, String, String)> {
        let inf = &self.inf;
        let object = call.first_child()?;
        let m = crate::api::method_name(call)?;
        let body = inf.method_body(&object, m.text())?;
        if !inf.dot_defined(&body) {
            return None;
        }
        let want = inf.body_info(&body).params.first().cloned().flatten()?;
        let colon = call.children_with_tokens().find(|e| e.kind() == COLON)?.text_range();
        let got = inf.expr_type(&object);
        if has_unknown(&want) || mentions_any(&want) || inf.exhausted.get() || inf.cast(&got, &want, 0) != Some(false) {
            return None;
        }
        Some((colon, crate::luacats::view(&got), crate::luacats::view(&want)))
    }

    /// `---@cast x T` on a local whose doc comment declares its type, when `T`
    /// does not fit that type: (declared, `T`).
    pub(crate) fn cast_mismatch(&self, d: usize, ty: &T) -> Option<(String, String)> {
        let inf = &self.inf;
        let decl = &inf.names.decls[d];
        let declared = match decl.kind {
            DeclKind::Local => inf.decl_stat(d).is_some_and(|st| stat_tags(inf.text, &st).iter().any(|t| matches!(t, Tag::Type { .. } | Tag::Class { .. }))),
            DeclKind::Param => true,
            _ => false,
        };
        if !declared {
            return None;
        }
        let def = inf.decl_type(d);
        if has_unknown(&def) || mentions_any(&def) || is(&def, "any") {
            return None;
        }
        // As LuaLS's `canCastType`: `T` must fit the declared type (a subclass,
        // one member of a union); nil is not checked.
        let fails = inf.cast(ty, &def, 0) == Some(false);
        (!inf.exhausted.get() && fails).then(|| (crate::luacats::view(&def), crate::luacats::view(ty)))
    }

    /// no-unknown: a local whose type nothing tells (`unknown`, not `any`).
    pub(crate) fn unknown_decl(&self, d: usize) -> bool {
        let t = self.inf.decl_type(d);
        !self.inf.exhausted.get() && is(&t, "unknown")
    }

    /// A local's type; `None` when it is not known.
    pub fn decl(&self, d: usize) -> Option<String> {
        let t = self.inf.decl_type(d);
        (!is(&t, "unknown") && !is(&t, "any")).then(|| crate::luacats::view(&t))
    }
}

/// The alias of an `---@enum` whose values are not literals.
const OPAQUE_ENUM: &str = "<enum>";

/// `---@enum A` on a table of literal values: `A` is one of the values
/// (`---@enum(key) A`: one of the keys). An enum with any other value is left
/// out (its type is not known).
fn enum_aliases(root: &SyntaxNode, text: &str) -> HashMap<String, T> {
    let mut out = HashMap::new();
    for stat in root.descendants().filter(|n| matches!(n.kind(), LOCAL_STAT | ASSIGN_STAT)) {
        let Some((name, key)) = stat_tags(text, &stat).into_iter().find_map(|t| match t {
            Tag::Enum { name, key } => Some((name, key)),
            _ => None,
        }) else {
            continue;
        };
        let Some(table) = stat.children().find(|c| c.kind() == EXPR_LIST).and_then(|l| l.first_child()).filter(|t| t.kind() == TABLE_EXPR) else { continue };
        let mut members = Vec::new();
        let mut known = true;
        for field in table.children() {
            let m = if key {
                match field.kind() {
                    FIELD_NAMED => field.children_with_tokens().filter_map(|x| x.into_token()).find(|t| t.kind() == NAME).map(|t| T::Str(t.text().to_string())),
                    FIELD_INDEXED => field.first_child().and_then(|k| literal_type(&k)),
                    _ => None,
                }
            } else {
                match field.kind() {
                    FIELD_NAMED | FIELD_INDEXED | FIELD_POS => field.children().last().and_then(|v| literal_type(&v)),
                    _ => None,
                }
            };
            match m {
                Some(m) => members.push(m),
                None => known = false,
            }
        }
        if known && !members.is_empty() {
            out.insert(name, union(members));
        } else if !key {
            // Values the type cannot spell (tables): a fresh table is none of them.
            out.insert(name, T::Generic(OPAQUE_ENUM.to_string(), Vec::new()));
        }
    }
    out
}

/// The literal type of a literal expression (`3`, `'x'`, `true`).
fn literal_type(e: &SyntaxNode) -> Option<T> {
    if e.kind() != LITERAL_EXPR {
        return None;
    }
    let t = e.first_token()?;
    match t.kind() {
        NUMBER => t.text().parse::<i64>().ok().map(T::Int),
        STRING => crate::util::unquote(t.text()).filter(|_| !t.text().contains('\\')).map(|s| T::Str(s.to_string())),
        TRUE_KW => Some(T::Bool(true)),
        FALSE_KW => Some(T::Bool(false)),
        _ => None,
    }
}

/// A decimal number literal with a fraction (`1.5`, not `2.0` or `1e3`).
fn fractional_literal(e: &SyntaxNode) -> bool {
    e.kind() == LITERAL_EXPR
        && e.first_token().is_some_and(|t| {
            let s = t.text();
            t.kind() == NUMBER && !s.starts_with("0x") && !s.starts_with("0X") && s.parse::<f64>().is_ok_and(|v| v.fract() != 0.0)
        })
}

/// Whether a fractional number fits none of the type's members while
/// `integer` is one of them.
fn float_misfits(t: &T) -> bool {
    fn members<'t>(t: &'t T, out: &mut Vec<&'t T>) {
        match t {
            T::Optional(i) => members(i, out),
            T::Union(ps) => ps.iter().for_each(|p| members(p, out)),
            t => out.push(t),
        }
    }
    let mut ms = Vec::new();
    members(t, &mut ms);
    ms.iter().any(|m| is(m, "integer"))
        && ms.iter().all(|m| match m {
            T::Name(n) => BASIC.contains(&n.as_str()) && !matches!(n.as_str(), "number" | "any" | "unknown"),
            T::Str(_) | T::Int(_) | T::Bool(_) => true,
            _ => false,
        })
}

/// `---@alias Name T` (with `---|` lines adding members).
fn doc_aliases(text: &str) -> HashMap<String, T> {
    let mut out = HashMap::new();
    let mut run: Vec<(TextSize, &str)> = Vec::new();
    let mut flush = |run: &mut Vec<(TextSize, &str)>| {
        if run.iter().any(|(_, l)| l.contains("@alias")) {
            let (tags, _) = crate::luacats::parse_lines(run.drain(..));
            let mut cur: Option<(String, Vec<T>)> = None;
            for t in tags {
                match t.tag {
                    Tag::Alias { name, ty, .. } => {
                        if let Some((n, parts)) = cur.take() {
                            out.insert(n, union(parts));
                        }
                        cur = Some((name, ty.into_iter().collect()));
                    }
                    Tag::AliasValue { ty, .. } => {
                        if let Some((_, parts)) = &mut cur {
                            parts.push(ty);
                        }
                    }
                    _ => {}
                }
            }
            if let Some((n, parts)) = cur {
                out.insert(n, union(parts));
            }
        }
        run.clear();
    };
    let mut at = 0usize;
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        let t = body.trim_start();
        if t.starts_with("---") {
            let lead = body.len() - t.len();
            run.push((TextSize::from((at + lead + 2) as u32), &t[2..]));
        } else {
            flush(&mut run);
        }
        at += line.len();
    }
    flush(&mut run);
    out
}

/// The `---@` tags of the comments a statement starts with.
fn stat_tags(text: &str, n: &SyntaxNode) -> Vec<Tag> {
    let mut lines = Vec::new();
    for e in n.children_with_tokens() {
        match e {
            rowan::NodeOrToken::Token(t) if t.kind() == COMMENT => {
                if let Some(rest) = text[t.text_range()].strip_prefix("--") {
                    lines.push((t.text_range().start() + TextSize::from(2), rest));
                }
            }
            rowan::NodeOrToken::Token(t) if t.kind() == WHITESPACE => {}
            _ => break,
        }
    }
    // `local x ---@type T`: a `---@type` after the statement on its line.
    let mut at = n.next_sibling_or_token();
    while let Some(rowan::NodeOrToken::Token(t)) = at {
        match t.kind() {
            WHITESPACE if !t.text().contains('\n') => at = t.next_sibling_or_token(),
            COMMENT => {
                let line = &text[t.text_range()];
                if line.starts_with("---") && line.contains("@type") {
                    lines.push((t.text_range().start() + TextSize::from(2), &line[2..]));
                }
                break;
            }
            _ => break,
        }
    }
    crate::luacats::parse_lines(lines).0.into_iter().map(|t| t.tag).collect()
}

/// The class a `---@class` tag of a statement declares.
fn class_tag(text: &str, st: &SyntaxNode) -> Option<String> {
    stat_tags(text, st).into_iter().find_map(|t| match t {
        Tag::Class { name, .. } => Some(name),
        _ => None,
    })
}

fn token_kind_type(t: &SyntaxToken) -> Option<T> {
    Some(match t.kind() {
        NIL_KW => name("nil"),
        TRUE_KW | FALSE_KW => name("boolean"),
        STRING | LONG_STRING => name("string"),
        // CfxLua: `` `WEAPON_PISTOL` `` is the name's joaat hash.
        HASH_STRING => name("integer"),
        NUMBER => {
            let s = t.text();
            let hex = s.starts_with("0x") || s.starts_with("0X");
            let float = if hex { s.contains(['.', 'p', 'P']) } else { s.contains(['.', 'e', 'E']) };
            name(if float { "number" } else { "integer" })
        }
        _ => return None,
    })
}

impl<'a> Infer<'a> {
    fn new(root: SyntaxNode, names: &'a Names, text: &'a str) -> Infer<'a> {
        // A class declared in several blocks of the file is one class.
        let mut classes: HashMap<String, crate::api::ClassDef> = HashMap::new();
        for c in crate::api::doc_classes(text) {
            match classes.get_mut(&c.name) {
                Some(have) => {
                    have.fields.extend(c.fields);
                    have.parents.extend(c.parents);
                    have.operators.extend(c.operators);
                    have.indexes.extend(c.indexes);
                    have.exact |= c.exact;
                }
                None => {
                    classes.insert(c.name.clone(), c);
                }
            }
        }
        let mut aliases = doc_aliases(text);
        aliases.extend(enum_aliases(&root, text));
        Infer { text, names, root, classes, aliases, depth: Cell::new(0), flow_budget: Cell::new(FLOW_BUDGET),
            exhausted: Cell::new(false),
            functions: std::cell::OnceCell::new(),
            written: std::cell::OnceCell::new(),
            typed_fields: std::cell::OnceCell::new(),
            decl_stats: RefCell::default(),
            decls: RefCell::default(),
            globals: RefCell::default(),
            bodies: RefCell::default(),
            exprs: RefCell::default(),
        }
    }

    /// Runs `f` one level deeper, or gives `unknown` past the cap.
    fn deeper(&self, f: impl FnOnce() -> T) -> T {
        let d = self.depth.get();
        if d >= MAX_DEPTH {
            return unknown();
        }
        self.depth.set(d + 1);
        let t = f();
        self.depth.set(d);
        t
    }

    fn decl_at(&self, r: TextRange) -> Option<usize> {
        self.names.decls.iter().position(|d| d.range == r && d.kind != DeclKind::SelfParam)
    }

    fn ref_at(&self, r: TextRange) -> Option<&crate::names::NameRef> {
        let i = self.names.refs.binary_search_by_key(&r.start(), |x| x.range.start()).ok()?;
        Some(&self.names.refs[i])
    }

    fn token_type(&self, tok: &SyntaxToken) -> Option<T> {
        let parent = tok.parent()?;
        if tok.kind() == FUNCTION_KW {
            return Some(if parent.kind() == FUNCTION_EXPR { self.expr_type(&parent) } else { name("function") });
        }
        if tok.kind() != NAME {
            return None;
        }
        Some(match parent.kind() {
            LOCAL_NAME | PARAM | NUMERIC_FOR_STAT | GENERIC_FOR_STAT => self.decl_type(self.decl_at(tok.text_range())?),
            LOCAL_FUNCTION_STAT | FUNC_NAME => self.function_name_type(&parent),
            NAME_EXPR => {
                let r = self.ref_at(tok.text_range());
                if r.is_some_and(|r| r.access == Access::Write)
                    && let Some(v) = self.assigned_value(&parent)
                {
                    return Some(v);
                }
                self.expr_type(&parent)
            }
            INDEX_EXPR => {
                if let Some(v) = self.assigned_value(&parent) {
                    return Some(v);
                }
                self.expr_type(&parent)
            }
            METHOD_CALL_EXPR => {
                let object = parent.first_child()?;
                let t = self.index_type(&object, &Key::Name(tok.text().to_string()));
                if is(&t, "unknown") { name("function") } else { t }
            }
            FIELD_NAMED => parent.children().last().map_or_else(unknown, |v| self.expr_type(&v)),
            _ => return None,
        })
    }

    /// `function f()`, `function a.b()`, `local function f()`.
    fn function_name_type(&self, n: &SyntaxNode) -> T {
        let stat = if n.kind() == FUNC_NAME { n.parent() } else { Some(n.clone()) };
        if let Some(stat) = stat {
            for tag in stat_tags(self.text, &stat) {
                if let Tag::Type { tys } = tag
                    && let Some(t) = tys.into_iter().next()
                {
                    return t;
                }
            }
        }
        name("function")
    }

    /// The value written by the assignment whose target is `target`.
    fn assigned_value(&self, target: &SyntaxNode) -> Option<T> {
        let list = target.parent().filter(|p| p.kind() == VAR_LIST)?;
        let stat = list.parent().filter(|p| p.kind() == ASSIGN_STAT)?;
        let i = list.children().position(|c| c == *target)?;
        let values: Vec<SyntaxNode> = stat.children().find(|c| c.kind() == EXPR_LIST).map(|l| l.children().collect()).unwrap_or_default();
        if i == 0 {
            for tag in stat_tags(self.text, &stat) {
                match tag {
                    Tag::Class { name: c, .. } => return Some(name(&c)),
                    Tag::Type { tys } if !tys.is_empty() => return Some(tys[0].clone()),
                    _ => {}
                }
            }
        }
        Some(self.value_at(&values, i))
    }

    /// The `i`th value of an expression list (the last expression spreads).
    fn value_at(&self, values: &[SyntaxNode], i: usize) -> T {
        let Some(last) = values.len().checked_sub(1) else { return name("nil") };
        if i < last {
            return self.expr_type(&values[i]);
        }
        let k = i - last;
        if let Some(t) = self.cast_after(&values[last]) {
            return if k == 0 { t } else { name("nil") };
        }
        match values[last].kind() {
            CALL_EXPR | METHOD_CALL_EXPR => self.call_returns(&values[last]).get(k).cloned().unwrap_or_else(|| name("nil")),
            VARARG_EXPR => self.vararg_type(&values[last]),
            _ if k == 0 => self.expr_type(&values[last]),
            _ => name("nil"),
        }
    }

    fn decl_type(&self, d: usize) -> T {
        if let Some(known) = self.decls.borrow().get(&d) {
            return known.clone().unwrap_or_else(unknown);
        }
        self.decls.borrow_mut().insert(d, None);
        let t = self.deeper(|| self.decl_type_inner(d));
        self.decls.borrow_mut().insert(d, Some(t.clone()));
        t
    }

    fn decl_type_inner(&self, d: usize) -> T {
        let decl = &self.names.decls[d];
        let Some(tok) = self.root.token_at_offset(decl.range.start()).right_biased() else { return unknown() };
        match decl.kind {
            DeclKind::LocalFunction => name("function"),
            DeclKind::Label => unknown(),
            DeclKind::SelfParam => self.self_type(&tok),
            DeclKind::Param => self.param_type(&tok),
            DeclKind::ForVar => self.for_var_type(&tok),
            DeclKind::Local => {
                let Some(stat) = tok.parent_ancestors().find(|a| a.kind() == LOCAL_STAT) else { return unknown() };
                let locals: Vec<SyntaxToken> =
                    stat.children().filter(|c| c.kind() == LOCAL_NAME).filter_map(|c| c.first_token()).collect();
                let i = locals.iter().position(|t| t.text_range() == decl.range).unwrap_or(0);
                if i == 0 {
                    for tag in stat_tags(self.text, &stat) {
                        match tag {
                            Tag::Class { name: c, .. } => return name(&c),
                            Tag::Type { tys } if !tys.is_empty() => return tys[0].clone(),
                            _ => {}
                        }
                    }
                } else {
                    for tag in stat_tags(self.text, &stat) {
                        if let Tag::Type { tys } = tag
                            && let Some(t) = tys.get(i)
                        {
                            return t.clone();
                        }
                    }
                }
                let values: Vec<SyntaxNode> = stat.children().find(|c| c.kind() == EXPR_LIST).map(|l| l.children().collect()).unwrap_or_default();
                if !values.is_empty() {
                    return self.value_at(&values, i);
                }
                // Declared empty: the first value written to it.
                for r in self.names.refs.iter().filter(|r| r.target == Target::Decl(d) && r.access == Access::Write) {
                    let Some(e) = self.root.token_at_offset(r.range.start()).right_biased().and_then(|t| t.parent()) else { continue };
                    if let Some(v) = self.assigned_value(&e) {
                        return v;
                    }
                }
                unknown()
            }
        }
    }

    /// A local read at `e`: the last write before it in the same or an
    /// enclosing block, narrowed by the checks around it (`if x then`,
    /// `x and x.y`, `if not x then return end`, `assert(x)`) and widened by
    /// writes inside earlier branches. Loops and closures are not followed back.
    fn read_type(&self, e: &SyntaxNode, d: usize) -> T {
        let mut effects: Vec<Effect> = Vec::new();
        let mut node = e.clone();
        let base = 'walk: loop {
            let Some(parent) = node.parent() else { break self.decl_type(d) };
            match parent.kind() {
                BLOCK => {
                    for c in [self.cast_of(&node, d), self.detached_cast(&node, d)].into_iter().flatten() {
                        effects.push(c);
                    }
                    let mut prev = node.prev_sibling();
                    while let Some(s) = prev {
                        let b = self.flow_budget.get();
                        if b == 0 {
                            self.exhausted.set(true);
                            break 'walk self.decl_type(d);
                        }
                        self.flow_budget.set(b - 1);
                        match self.stat_effect(&s, d) {
                            Some(Effect::Set(t)) => break 'walk t,
                            Some(x) => effects.push(x),
                            None => {}
                        }
                        for c in [self.cast_of(&s, d), self.detached_cast(&s, d)].into_iter().flatten() {
                            effects.push(c);
                        }
                        prev = s.prev_sibling();
                    }
                    let Some(owner) = parent.parent() else { break self.decl_type(d) };
                    match owner.kind() {
                        IF_STAT | ELSEIF_CLAUSE | WHILE_STAT if let Some(t) = owner.first_child().and_then(|c| self.type_check(&c, d)) => {
                            effects.push(Effect::Narrow(t))
                        }
                        IF_STAT | ELSEIF_CLAUSE | WHILE_STAT if let Some(c) = owner.first_child().filter(|c| self.checks(c, d) == Some(true)) => {
                            effects.push(self.guard(&c, d))
                        }
                        IF_STAT | ELSEIF_CLAUSE | WHILE_STAT if let Some(f) = owner.first_child().and_then(|c| self.falsy_when(&c, d, true)) => {
                            effects.push(f)
                        }
                        ELSE_CLAUSE => {
                            let conds: Vec<SyntaxNode> = owner
                                .parent()
                                .into_iter()
                                .flat_map(|i| {
                                    std::iter::once(i.first_child()).chain(i.children().filter(|c| c.kind() == ELSEIF_CLAUSE).map(|c| c.first_child()))
                                })
                                .flatten()
                                .collect();
                            if let Some(c) = conds.iter().find(|c| self.checks(c, d) == Some(false)) {
                                effects.push(self.guard(c, d));
                            } else if let Some(f) = conds.iter().find_map(|c| self.falsy_when(c, d, false)) {
                                effects.push(f);
                            }
                        }
                        FUNC_BODY | SOURCE_FILE => break self.decl_type(d),
                        _ => {}
                    }
                }
                BINARY_EXPR => {
                    let op = parent.children_with_tokens().filter_map(|x| x.into_token()).find(|t| matches!(t.kind(), AND_KW | OR_KW)).map(|t| t.kind());
                    let left = parent.first_child();
                    if left.as_ref() != Some(&node)
                        && let Some(l) = left
                    {
                        match (op, self.checks(&l, d)) {
                            // `type(x) == 'string' and x`.
                            (Some(AND_KW), _) if let Some(t) = self.type_check(&l, d) => effects.push(Effect::Narrow(t)),
                            (Some(AND_KW), Some(true)) | (Some(OR_KW), Some(false)) => effects.push(self.guard(&l, d)),
                            // `not x and x`, `x or x`.
                            (Some(AND_KW), _) if let Some(f) = self.falsy_when(&l, d, true) => effects.push(f),
                            (Some(OR_KW), _) if let Some(f) = self.falsy_when(&l, d, false) => effects.push(f),
                            _ => {}
                        }
                    }
                }
                // In an `elseif` (its condition or block) the conditions before it were false.
                ELSEIF_CLAUSE => {
                    let earlier = parent.parent().into_iter().flat_map(|i| {
                        let before = parent.text_range().start();
                        std::iter::once(i.first_child())
                            .chain(i.children().filter(move |c| c.kind() == ELSEIF_CLAUSE && c.text_range().start() < before).map(|c| c.first_child()))
                    });
                    let earlier: Vec<SyntaxNode> = earlier.flatten().collect();
                    if let Some(c) = earlier.iter().find(|c| self.checks(c, d) == Some(false)) {
                        effects.push(self.guard(c, d));
                    } else if let Some(f) = earlier.iter().find_map(|c| self.falsy_when(c, d, false)) {
                        effects.push(f);
                    }
                }
                FUNC_BODY => break self.decl_type(d),
                _ => {}
            }
            node = parent;
        };
        let mut t = base;
        for x in effects.into_iter().rev() {
            t = match x {
                Effect::NonNil => non_nil(t),
                Effect::Truthy => truthy(t),
                Effect::Falsy => falsy(t, false),
                Effect::Nil => falsy(t, true),
                Effect::Also(v) => union(std::iter::once(t).chain(v).collect()),
                Effect::Fill(v) => union(std::iter::once(non_nil(t)).chain(v).collect()),
                Effect::Narrow(n) => {
                    // The members of the known type that the check keeps.
                    let fits = |m: &T| *m == n || is(&n, "number") && numeric(m).is_some();
                    let parts: Vec<T> = match non_nil(t.clone()) {
                        T::Union(ps) => ps,
                        m => vec![m],
                    };
                    let kept: Vec<T> = parts.into_iter().filter(fits).collect();
                    if kept.is_empty() { n } else { union(kept) }
                }
                Effect::Cast(ops) => {
                    let mut t = t;
                    for (op, c) in ops {
                        t = match op {
                            '+' => union(vec![t, c]),
                            '-' => {
                                let gone = |p: &T| *p == c;
                                match t {
                                    T::Optional(inner) if is(&c, "nil") => *inner,
                                    T::Union(parts) => union(parts.into_iter().filter(|p| !gone(p)).collect()),
                                    t => t,
                                }
                            }
                            _ => c,
                        };
                    }
                    t
                }
                Effect::Set(t) => t,
            };
        }
        t
    }

    /// Whether `cond` being true means local `d` is set (`Some(true)`: `x`,
    /// `x ~= nil`, `x and ...`) or nil (`Some(false)`: `not x`, `x == nil`).
    fn checks(&self, cond: &SyntaxNode, d: usize) -> Option<bool> {
        let is_d = |n: &SyntaxNode| {
            n.kind() == NAME_EXPR && n.first_token().and_then(|t| self.ref_at(t.text_range())).is_some_and(|r| r.target == Target::Decl(d))
        };
        match cond.kind() {
            PAREN_EXPR => self.checks(&cond.first_child()?, d),
            NAME_EXPR => is_d(cond).then_some(true),
            UNARY_EXPR if cond.first_token().is_some_and(|t| t.kind() == NOT_KW) => self.checks(&cond.first_child()?, d).map(|b| !b),
            BINARY_EXPR => {
                let mut kids = cond.children();
                let (l, r) = (kids.next()?, kids.next()?);
                let op = cond.children_with_tokens().filter_map(|x| x.into_token()).find(|t| !matches!(t.kind(), WHITESPACE | COMMENT))?.kind();
                let nil_side = |a: &SyntaxNode, b: &SyntaxNode| is_d(a) && b.kind() == LITERAL_EXPR && b.first_token().is_some_and(|t| t.kind() == NIL_KW);
                match op {
                    AND_KW => match (self.checks(&l, d), self.checks(&r, d)) {
                        (Some(true), _) | (_, Some(true)) => Some(true),
                        (Some(false), Some(false)) => Some(false),
                        _ => None,
                    },
                    // `not x or ...` false: every side is false. `type(x) == 'a' or type(x) == 'b'`: x is set.
                    OR_KW => match (self.checks(&l, d), self.checks(&r, d)) {
                        (Some(false), _) | (_, Some(false)) => Some(false),
                        (Some(true), Some(true)) => Some(true),
                        _ => None,
                    },
                    NEQ if nil_side(&l, &r) || nil_side(&r, &l) => Some(true),
                    EQ2 if nil_side(&l, &r) || nil_side(&r, &l) => Some(false),
                    // `type(x) == 'table'` true, `type(x) ~= 'table'` false: x is set.
                    EQ2 | NEQ if let Some(lit) = self.type_test(&l, &r, d).or_else(|| self.type_test(&r, &l, d)) => {
                        Some((op == EQ2) != (lit == "nil"))
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// What `cond` having the value `truth` says of local `d` being falsy:
    /// `Falsy` (`x` false, `not x` true), `Nil` (`x == nil` true).
    fn falsy_when(&self, cond: &SyntaxNode, d: usize, truth: bool) -> Option<Effect> {
        let is_d = |n: &SyntaxNode| {
            n.kind() == NAME_EXPR && n.first_token().and_then(|t| self.ref_at(t.text_range())).is_some_and(|r| r.target == Target::Decl(d))
        };
        match cond.kind() {
            PAREN_EXPR => self.falsy_when(&cond.first_child()?, d, truth),
            NAME_EXPR => (is_d(cond) && !truth).then_some(Effect::Falsy),
            UNARY_EXPR if cond.first_token().is_some_and(|t| t.kind() == NOT_KW) => self.falsy_when(&cond.first_child()?, d, !truth),
            BINARY_EXPR => {
                let mut kids = cond.children();
                let (l, r) = (kids.next()?, kids.next()?);
                let op = cond.children_with_tokens().filter_map(|x| x.into_token()).find(|t| !matches!(t.kind(), WHITESPACE | COMMENT))?.kind();
                let nil_side = |a: &SyntaxNode, b: &SyntaxNode| is_d(a) && b.kind() == LITERAL_EXPR && b.first_token().is_some_and(|t| t.kind() == NIL_KW);
                match op {
                    // Both sides true, or both false.
                    AND_KW if truth => self.falsy_when(&l, d, true).or_else(|| self.falsy_when(&r, d, true)),
                    OR_KW if !truth => self.falsy_when(&l, d, false).or_else(|| self.falsy_when(&r, d, false)),
                    EQ2 if truth && (nil_side(&l, &r) || nil_side(&r, &l)) => Some(Effect::Nil),
                    NEQ if !truth && (nil_side(&l, &r) || nil_side(&r, &l)) => Some(Effect::Nil),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// What a condition that `checks` local `d` guarantees: truthiness, or
    /// only not-nil when it compares with nil (`x ~= nil` keeps `false`).
    fn guard(&self, cond: &SyntaxNode, d: usize) -> Effect {
        fn by_truth(inf: &Infer, c: &SyntaxNode, d: usize) -> bool {
            match c.kind() {
                PAREN_EXPR | UNARY_EXPR => c.first_child().is_some_and(|i| by_truth(inf, &i, d)),
                NAME_EXPR => true,
                BINARY_EXPR => c.children().any(|k| inf.checks(&k, d).is_some() && by_truth(inf, &k, d)),
                _ => false,
            }
        }
        if by_truth(self, cond, d) { Effect::Truthy } else { Effect::NonNil }
    }

    /// `---@cast` of local `d` in the comments `s` starts with.
    fn cast_of(&self, s: &SyntaxNode, d: usize) -> Option<Effect> {
        self.cast_in(stat_tags(self.text, s), d)
    }

    /// `---@cast` lines standing alone (blank lines around them) right before
    /// the statement `s`.
    fn detached_cast(&self, s: &SyntaxNode, d: usize) -> Option<Effect> {
        let mut lines = Vec::new();
        let mut at = s.prev_sibling_or_token();
        while let Some(rowan::NodeOrToken::Token(t)) = at {
            match t.kind() {
                COMMENT => {
                    if let Some(rest) = self.text[t.text_range()].strip_prefix("--") {
                        lines.push((t.text_range().start() + TextSize::from(2), rest));
                    }
                }
                WHITESPACE => {}
                _ => break,
            }
            at = t.prev_sibling_or_token();
        }
        if !lines.iter().any(|(_, l)| l.contains("@cast")) {
            return None;
        }
        lines.reverse();
        self.cast_in(crate::luacats::parse_lines(lines).0.into_iter().map(|t| t.tag).collect(), d)
    }

    fn cast_in(&self, tags: Vec<Tag>, d: usize) -> Option<Effect> {
        let decl = &self.names.decls[d];
        let n = &self.text[decl.range];
        let ops: Vec<(char, T)> = tags
            .into_iter()
            .filter_map(|t| match t {
                Tag::Cast { name, ops } if name == n => Some(ops),
                _ => None,
            })
            .flatten()
            .collect();
        (!ops.is_empty()).then_some(Effect::Cast(ops))
    }

    /// `type(x) == 'string'` (either side): the type named.
    /// `type(x)` compared with the string `lit`: that string.
    fn type_test(&self, call: &SyntaxNode, lit: &SyntaxNode, d: usize) -> Option<String> {
        if call.kind() != CALL_EXPR || &self.text[call.first_child()?.text_range()] != "type" {
            return None;
        }
        let arg = call.children().find(|c| c.kind() == ARG_LIST)?.first_child()?;
        let is_d = arg.kind() == NAME_EXPR && arg.first_token().and_then(|t| self.ref_at(t.text_range())).is_some_and(|r| r.target == Target::Decl(d));
        if !is_d || lit.kind() != LITERAL_EXPR {
            return None;
        }
        crate::util::unquote(&self.text[lit.text_range()]).map(str::to_string)
    }

    fn type_check(&self, cond: &SyntaxNode, d: usize) -> Option<T> {
        if cond.kind() == PAREN_EXPR {
            return self.type_check(&cond.first_child()?, d);
        }
        if cond.kind() != BINARY_EXPR {
            return None;
        }
        let mut kids = cond.children();
        let (l, r) = (kids.next()?, kids.next()?);
        let op = cond.children_with_tokens().filter_map(|x| x.into_token()).find(|t| !matches!(t.kind(), WHITESPACE | COMMENT))?.kind();
        // `x ~= nil and type(x) == 'number'`: both sides hold.
        if op == AND_KW {
            return self.type_check(&l, d).or_else(|| self.type_check(&r, d));
        }
        if op != EQ2 {
            return None;
        }
        let side = |call: &SyntaxNode, lit: &SyntaxNode| -> Option<T> {
            if call.kind() != CALL_EXPR || &self.text[call.first_child()?.text_range()] != "type" {
                return None;
            }
            let arg = call.children().find(|c| c.kind() == ARG_LIST)?.first_child()?;
            let is_d = arg.kind() == NAME_EXPR && arg.first_token().and_then(|t| self.ref_at(t.text_range())).is_some_and(|r| r.target == Target::Decl(d));
            if !is_d {
                return None;
            }
            crate::util::unquote(&self.text[lit.text_range()]).map(name)
        };
        side(&l, &r).or_else(|| side(&r, &l))
    }

    /// What statement `s` does to local `d` (see [`Effect`]).
    fn stat_effect(&self, s: &SyntaxNode, d: usize) -> Option<Effect> {
        let r = s.text_range();
        let from = self.names.refs.partition_point(|x| x.range.start() < r.start());
        let writes: Vec<&crate::names::NameRef> = self.names.refs[from..]
            .iter()
            .take_while(|x| x.range.end() <= r.end())
            .filter(|x| x.target == Target::Decl(d) && x.access == Access::Write)
            .collect();
        let declares = self.names.decls[d].range;
        if s.kind() == LOCAL_STAT && r.contains_range(declares) {
            return Some(Effect::Set(self.decl_type(d)));
        }
        let value_of = |w: &crate::names::NameRef| {
            let e = self.root.token_at_offset(w.range.start()).right_biased().and_then(|t| t.parent())?;
            self.assigned_value(&e)
        };
        match s.kind() {
            FUNCTION_STAT if !writes.is_empty() => Some(Effect::Set(name("function"))),
            ASSIGN_STAT => {
                // The last write of the statement is what stays.
                let w = writes.last()?;
                Some(value_of(w).map_or_else(|| Effect::Set(unknown()), Effect::Set))
            }
            IF_STAT => {
                let cond = s.first_child()?;
                let then = s.children().find(|c| c.kind() == BLOCK)?;
                let simple = !s.children().any(|c| matches!(c.kind(), ELSEIF_CLAUSE | ELSE_CLAUSE));
                if simple && self.checks(&cond, d) == Some(false) {
                    let last = then.children().last();
                    let leaves = last.as_ref().is_some_and(|l| {
                        matches!(l.kind(), RETURN_STAT | BREAK_STAT | GOTO_STAT)
                            || l.kind() == CALL_STAT && l.first_child().and_then(|c| c.first_child()).is_some_and(|c| &self.text[c.text_range()] == "error")
                    });
                    if leaves {
                        return Some(self.guard(&cond, d));
                    }
                    if !writes.is_empty() {
                        return Some(Effect::Fill(writes.iter().filter_map(|w| value_of(w)).collect()));
                    }
                }
                (!writes.is_empty()).then(|| Effect::Also(writes.iter().filter_map(|w| value_of(w)).collect()))
            }
            CALL_STAT => {
                let call = s.first_child()?;
                let callee = call.first_child()?;
                if &self.text[callee.text_range()] == "assert"
                    && let Some(a) = call.children().find(|c| c.kind() == ARG_LIST).and_then(|a| a.first_child())
                    && self.checks(&a, d) == Some(true)
                {
                    return Some(self.guard(&a, d));
                }
                None
            }
            _ => (!writes.is_empty()).then(|| Effect::Also(writes.iter().filter_map(|w| value_of(w)).collect())),
        }
    }

    /// `self` in `function obj:m()`: the type of `obj`.
    fn self_type(&self, colon: &SyntaxToken) -> T {
        let Some(func_name) = colon.parent().filter(|p| p.kind() == FUNC_NAME) else { return unknown() };
        let parts: Vec<SyntaxToken> = func_name.children_with_tokens().filter_map(|e| e.into_token()).filter(|t| t.kind() == NAME).collect();
        let Some(first) = parts.first() else { return unknown() };
        if parts.len() > 2 {
            return unknown();
        }
        match self.ref_at(first.text_range()).map(|r| r.target) {
            Some(Target::Decl(d)) => self.decl_type(d),
            Some(Target::Global) => self.global_type(first.text()),
            None => unknown(),
        }
    }

    /// A parameter: its `---@param` type, else `any`.
    fn param_type(&self, tok: &SyntaxToken) -> T {
        let Some(body) = tok.parent_ancestors().find(|a| a.kind() == FUNC_BODY) else { return name("any") };
        let Some(holder) = body.parent() else { return name("any") };
        let stat = match holder.kind() {
            FUNCTION_STAT | LOCAL_FUNCTION_STAT => Some(holder.clone()),
            FUNCTION_EXPR => holder.ancestors().find(|a| matches!(a.kind(), LOCAL_STAT | ASSIGN_STAT)).filter(|s| {
                s.children().find(|c| c.kind() == EXPR_LIST).and_then(|l| l.first_child()).is_some_and(|f| f == holder)
            }),
            _ => None,
        };
        let pname = tok.text();
        let index = body
            .children()
            .find(|c| c.kind() == PARAM_LIST)
            .and_then(|l| l.children().filter(|p| p.kind() == PARAM).position(|p| p.first_token().as_ref() == Some(tok)));
        if let Some(stat) = &stat {
            let tags = stat_tags(self.text, stat);
            for tag in &tags {
                if let Tag::Param { name: n, ty: Some(t), optional, .. } = tag
                    && n == pname
                {
                    return if *optional { T::Optional(Box::new(t.clone())) } else { t.clone() };
                }
            }
            // `---@type fun(x: number)` on the function.
            for tag in &tags {
                if let Tag::Type { tys } = tag
                    && let (Some(T::Fun(f)), Some(i)) = (tys.first(), index)
                    && let Some(t) = f.params.get(i).and_then(|p| p.ty.clone())
                {
                    return t;
                }
            }
            // `---@overload fun(a: string)` / `fun(a: number)`: any of them.
            if let Some(i) = index {
                let from: Vec<T> = tags
                    .iter()
                    .filter_map(|t| match t {
                        Tag::Overload(T::Fun(f)) => f.params.get(i).and_then(|p| p.ty.clone()),
                        _ => None,
                    })
                    .collect();
                if !from.is_empty() {
                    return union(from);
                }
            }
        }
        // An override: `function B:f(x)` takes `x` from a parent class's `A:f`.
        if holder.kind() == FUNCTION_STAT
            && let Some(i) = index
            && let Some(t) = self.inherited_param(&holder, &body, i)
        {
            return t;
        }
        // A function passed where a `fun(...)` is expected.
        if holder.kind() == FUNCTION_EXPR
            && let Some(i) = index
            && let Some(T::Fun(f)) = self.expected_type(&holder)
            && let Some(t) = f.params.get(i).and_then(|p| p.ty.clone())
        {
            return t;
        }
        name("any")
    }

    /// The type the position of an expression asks for: a `fun(...)`
    /// parameter of the function called, the value type of a `---@type`
    /// table, the declared type of the local.
    fn expected_type(&self, e: &SyntaxNode) -> Option<T> {
        let parent = e.parent()?;
        match parent.kind() {
            ARG_LIST => {
                let call = parent.parent()?;
                let mut i = parent.children().position(|c| c == *e)?;
                let mut callee = call.first_child()?;
                // `pcall(work, fn)`, `xpcall(work, handler, fn)`.
                let args: Vec<SyntaxNode> = parent.children().collect();
                match &self.text[callee.text_range()] {
                    "pcall" if i >= 1 => {
                        callee = args[0].clone();
                        i -= 1;
                    }
                    "xpcall" if i >= 2 => {
                        callee = args[0].clone();
                        i -= 2;
                    }
                    _ => {}
                }
                let info = if call.kind() == METHOD_CALL_EXPR {
                    let m = crate::api::method_name(&call)?;
                    // `function C.f(a, b)` called as `C:f(b)`: the object is `a`.
                    if self.method_body(&callee, m.text()).is_some_and(|b| self.dot_defined(&b)) {
                        i += 1;
                    }
                    self.method_info(&callee, m.text())?
                } else {
                    self.callee_info(&callee)?
                };
                info.params.get(i).cloned().flatten()
            }
            FIELD_NAMED | FIELD_INDEXED | FIELD_POS => {
                let ctor = parent.parent()?;
                let outer = self.expected_type(&ctor)?;
                let key = match parent.kind() {
                    FIELD_NAMED => Key::Name(parent.children_with_tokens().filter_map(|x| x.into_token()).find(|t| t.kind() == NAME)?.text().to_string()),
                    // `{ f }` is `[1] = f`.
                    FIELD_POS => Key::Index(name("integer")),
                    _ => Key::Index(unknown()),
                };
                self.member_of_type(&outer, &key)
            }
            EXPR_LIST => {
                let stat = parent.parent()?;
                // `return function(x) end` under `---@return fun(x: integer)`.
                if stat.kind() == RETURN_STAT {
                    let i = parent.children().position(|c| c == *e)?;
                    if let Some(t) = stat_tags(self.text, &stat).into_iter().find_map(|t| match t {
                        Tag::Type { tys } => tys.into_iter().nth(i),
                        _ => None,
                    }) {
                        return Some(t);
                    }
                    let body = stat.ancestors().find(|a| a.kind() == FUNC_BODY)?;
                    let holder = self.doc_stat_of(&body)?;
                    return stat_tags(self.text, &holder)
                        .into_iter()
                        .filter_map(|t| match t {
                            Tag::Return { rets, .. } => Some(rets),
                            _ => None,
                        })
                        .flatten()
                        .nth(i)
                        .map(|r| r.ty);
                }
                if !matches!(stat.kind(), LOCAL_STAT | ASSIGN_STAT) || parent.first_child().as_ref() != Some(e) {
                    return None;
                }
                stat_tags(self.text, &stat).into_iter().find_map(|t| match t {
                    Tag::Type { tys } => tys.into_iter().next(),
                    _ => None,
                })
            }
            _ => None,
        }
    }

    /// A call of a function of the file with `---@overload`s: the returns of
    /// the first overload, then the function itself, that the arguments fit
    /// (count and types). None without overloads or when none fits.
    fn overload_returns(&self, callee: &SyntaxNode, args: &[SyntaxNode]) -> Option<Vec<T>> {
        let body = self.function_body(callee)?;
        let stat = self.doc_stat_of(&body)?;
        let tags = stat_tags(self.text, &stat);
        let mut candidates: Vec<FunType> = tags
            .iter()
            .filter_map(|t| match t {
                Tag::Overload(T::Fun(f)) => Some((**f).clone()),
                _ => None,
            })
            .collect();
        if candidates.is_empty() {
            return None;
        }
        // The function itself, as a `fun` of its parameters and returns.
        let info = self.body_info(&body);
        let names: Vec<(String, bool)> = body
            .children()
            .find(|c| c.kind() == PARAM_LIST)
            .map(|l| {
                l.children()
                    .filter(|p| p.kind() == PARAM)
                    .filter_map(|p| p.first_token())
                    .map(|t| {
                        let optional = tags.iter().any(|g| matches!(g, Tag::Param { name, optional: true, .. } if name == t.text()));
                        (t.text().to_string(), optional)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let own = FunType {
            params: names
                .into_iter()
                .zip(info.params.iter())
                .map(|((n, optional), ty)| crate::luacats::FunParam { name: n, ty: ty.clone(), optional })
                .collect(),
            rets: info.rets.iter().map(|t| crate::luacats::FunRet { ty: t.clone(), name: None, optional: false }).collect(),
            is_async: false,
        };
        candidates.push(own);
        let gens = generics(&tags);
        // A literal argument as its literal type: `'y'` does not fit `a: 'x'`.
        let literal = |a: &SyntaxNode| -> Option<T> {
            let tok = a.first_token().filter(|_| a.kind() == LITERAL_EXPR)?;
            match tok.kind() {
                STRING => Some(T::Str(crate::util::unquote(tok.text())?.to_string())),
                NUMBER => tok.text().parse::<i64>().ok().map(T::Int),
                TRUE_KW => Some(T::Bool(true)),
                FALSE_KW => Some(T::Bool(false)),
                _ => None,
            }
        };
        let arg_types: Vec<T> = args.iter().map(|a| literal(a).unwrap_or_else(|| self.expr_type(a))).collect();
        let literal_mismatch = |a: &T, p: &T| match (a, p) {
            (T::Str(x), T::Str(y)) => x != y,
            (T::Int(x), T::Int(y)) => x != y,
            (T::Bool(x), T::Bool(y)) => x != y,
            (T::Str(_) | T::Int(_) | T::Bool(_), T::Union(parts)) => {
                parts.iter().all(|q| matches!(q, T::Str(_) | T::Int(_) | T::Bool(_)) && *q != *a)
            }
            _ => false,
        };
        let fits = |f: &FunType| {
            let vararg = f.params.last().is_some_and(|p| p.name == "...");
            let fixed = f.params.len() - usize::from(vararg);
            let required = f.params.iter().filter(|p| p.name != "..." && !p.optional && !p.ty.as_ref().is_some_and(|t| matches!(t, T::Optional(_)))).count();
            if arg_types.len() < required || (!vararg && arg_types.len() > fixed) {
                return false;
            }
            arg_types.iter().zip(&f.params).all(|(a, p)| match &p.ty {
                // A generic takes anything.
                Some(T::Name(g)) if gens.contains(g) => true,
                Some(pt) if p.name != "..." => !literal_mismatch(a, pt) && self.cast(&widen(a), pt, 0) != Some(false),
                _ => true,
            })
        };
        let f = candidates.iter().find(|f| fits(f))?;
        let chosen = Callee { params: f.params.iter().map(|p| p.ty.clone()).collect(), rets: fun_rets(f), generics: gens.clone() };
        Some(self.apply(&chosen, args))
    }

    /// Parameter `i` of the same method on the class of `function B:f()` or a
    /// parent of it (`function A:f(x)` with `---@param x number`), when typed.
    fn inherited_param(&self, stat: &SyntaxNode, body: &SyntaxNode, i: usize) -> Option<T> {
        let fname = stat.children().find(|c| c.kind() == FUNC_NAME)?;
        let parts: Vec<SyntaxToken> = fname.children_with_tokens().filter_map(|e| e.into_token()).filter(|t| t.kind() == NAME).collect();
        let [object, method] = parts.as_slice() else { return None };
        let class = match self.ref_at(object.text_range()).map(|r| r.target) {
            Some(Target::Decl(d)) => match non_nil(self.decl_type(d)) {
                T::Name(c) => c,
                _ => return None,
            },
            _ => object.text().to_string(),
        };
        // The class itself too: `---@type A` on `a` and `function a:f(x)`.
        self.class_def(&class)?;
        let mut todo = vec![class];
        let mut seen: Vec<String> = Vec::new();
        while let Some(p) = todo.pop() {
            if seen.contains(&p) || seen.len() > 16 {
                continue;
            }
            if let Some(other) = self.file_functions().get(&format!("{p}.{}", method.text())).filter(|b| *b != body)
                && let Some(tok) = other
                    .children()
                    .find(|c| c.kind() == PARAM_LIST)
                    .and_then(|l| l.children().filter(|c| c.kind() == PARAM).nth(i))
                    .and_then(|p| p.first_token())
                && tok.kind() == NAME
            {
                // Bounded: classes that inherit each other lead back here.
                let t = self.deeper(|| self.param_type(&tok));
                if !is(&t, "any") && !is(&t, "unknown") {
                    return Some(t);
                }
            }
            if let Some(def) = self.class_def(&p) {
                todo.extend(def.parents.iter().cloned());
            }
            seen.push(p);
        }
        None
    }

    /// The statement whose doc comment a function body takes: its own
    /// `function` statement, or the `local f = function` holding it.
    fn doc_stat_of(&self, body: &SyntaxNode) -> Option<SyntaxNode> {
        let h = body.parent()?;
        match h.kind() {
            FUNCTION_STAT | LOCAL_FUNCTION_STAT => Some(h),
            FUNCTION_EXPR => h.ancestors().find(|a| matches!(a.kind(), LOCAL_STAT | ASSIGN_STAT)).filter(|s| {
                s.children().find(|c| c.kind() == EXPR_LIST).and_then(|l| l.first_child()).is_some_and(|f| f == h)
            }),
            _ => None,
        }
    }

    /// `...` inside a function whose doc says `---@param ... T`: `T`.
    fn vararg_type(&self, e: &SyntaxNode) -> T {
        let Some(stat) = e.ancestors().find(|a| a.kind() == FUNC_BODY).and_then(|b| self.doc_stat_of(&b)) else { return unknown() };
        stat_tags(self.text, &stat)
            .into_iter()
            .find_map(|t| match t {
                Tag::Param { name, ty: Some(ty), .. } if name == "..." => Some(ty),
                _ => None,
            })
            .unwrap_or_else(unknown)
    }

    /// Numeric `for` variables are integers; `pairs` / `ipairs` give the
    /// table's keys and values.
    fn for_var_type(&self, tok: &SyntaxToken) -> T {
        let Some(stat) = tok.parent() else { return unknown() };
        if stat.kind() == NUMERIC_FOR_STAT {
            return name("integer");
        }
        if stat.kind() != GENERIC_FOR_STAT {
            return unknown();
        }
        let vars: Vec<SyntaxToken> =
            stat.children_with_tokens().filter_map(|e| e.into_token()).take_while(|t| t.kind() != IN_KW).filter(|t| t.kind() == NAME).collect();
        let i = vars.iter().position(|t| t == tok).unwrap_or(0);
        let exprs: Vec<SyntaxNode> = stat.children().find(|c| c.kind() == EXPR_LIST).map(|l| l.children().collect()).unwrap_or_default();
        let Some(call) = exprs.first().cloned() else { return unknown() };
        // `for k, v in f, t`: the iterator and its state given directly.
        if exprs.len() >= 2 {
            let Some(info) = self.callee_info(&call) else { return unknown() };
            return self.iterate(&info, &self.expr_type(&exprs[1]), i);
        }
        if call.kind() != CALL_EXPR {
            let Some(info) = self.callee_info(&call) else { return unknown() };
            return self.iterate(&info, &unknown(), i);
        }
        let callee = call.first_child().map(|c| self.text[c.text_range()].to_string()).unwrap_or_default();
        // An iterator factory of the file (`---@return fun(...)`).
        if let Some(c) = call.first_child()
            && let Some(info) = self.callee_info(&c)
        {
            let args: Vec<SyntaxNode> = call.children().find(|c| c.kind() == ARG_LIST).map(|a| a.children().collect()).unwrap_or_default();
            let rets = self.bound_returns(&info, &args, false);
            if let Some(iter) = rets.first().and_then(fun_info) {
                let state = rets.get(1).cloned().unwrap_or_else(unknown);
                let mut iter = iter;
                iter.generics = info.generics.clone();
                return self.iterate(&iter, &state, i);
            }
        }
        let Some(arg) = call.children().find(|c| c.kind() == ARG_LIST).and_then(|a| a.first_child()) else { return unknown() };
        let (k, v) = self.key_value_types(&arg);
        match (callee.as_str(), i) {
            ("ipairs", 0) => name("integer"),
            ("ipairs", 1) => v.1,
            ("pairs", 0) => k,
            ("pairs", 1) => v.0,
            _ => unknown(),
        }
    }

    /// The `i`th value an iterator gives over `state`.
    fn iterate(&self, iter: &Callee, state: &T, i: usize) -> T {
        let mut map = HashMap::new();
        // Names the iterator's own types leave open bind from the state.
        let mut gens = iter.generics.clone();
        if gens.is_empty() {
            for p in iter.params.iter().flatten() {
                open_names(p, &mut gens, self);
            }
        }
        if let Some(Some(p)) = iter.params.first() {
            bind(p, &self.expand(state), &gens, &mut map);
        }
        let t = iter.rets.get(i).map_or_else(|| name("nil"), |t| subst(t, &map, &gens, true));
        if i == 0 { non_nil(t) } else { t }
    }

    /// A table's key type and value types (`(all values, positional values)`).
    fn key_value_types(&self, e: &SyntaxNode) -> (T, (T, T)) {
        let t = self.expr_type(e);
        match self.expand(&t) {
            T::Generic(n, args) if n == "table" && args.len() == 2 => return (args[0].clone(), (args[1].clone(), args[1].clone())),
            T::Array(inner) => return (name("integer"), ((*inner).clone(), (*inner).clone())),
            _ => {}
        }
        let Some(ctor) = self.constructor(e) else { return (unknown(), (unknown(), unknown())) };
        let mut keys = Vec::new();
        let (mut all, mut pos) = (Vec::new(), Vec::new());
        for f in ctor.children() {
            let v = f.children().last().map(|v| self.expr_type(&v)).unwrap_or_else(unknown);
            match f.kind() {
                FIELD_NAMED => keys.push(name("string")),
                FIELD_POS => {
                    keys.push(name("integer"));
                    pos.push(v.clone());
                }
                FIELD_INDEXED => keys.push(f.first_child().map(|k| self.expr_type(&k)).unwrap_or_else(unknown)),
                _ => continue,
            }
            all.push(v);
        }
        let opt = |v: Vec<T>| if v.is_empty() { unknown() } else { union(v) };
        (opt(keys), (opt(all), opt(pos)))
    }

    /// The table constructor an expression holds (a literal, or a local or
    /// global set to one).
    fn constructor(&self, e: &SyntaxNode) -> Option<SyntaxNode> {
        match e.kind() {
            TABLE_EXPR => Some(e.clone()),
            PAREN_EXPR => self.constructor(&e.first_child()?),
            NAME_EXPR => {
                let tok = e.first_token()?;
                let Target::Decl(d) = self.ref_at(tok.text_range())?.target else { return None };
                let decl = &self.names.decls[d];
                if decl.kind != DeclKind::Local {
                    return None;
                }
                let dt = self.root.token_at_offset(decl.range.start()).right_biased()?;
                let stat = dt.parent_ancestors().find(|a| a.kind() == LOCAL_STAT)?;
                let i = stat.children().filter(|c| c.kind() == LOCAL_NAME).position(|c| c.text_range().start() == decl.range.start())?;
                let v = stat.children().find(|c| c.kind() == EXPR_LIST)?.children().nth(i)?;
                (v.kind() == TABLE_EXPR).then_some(v)
            }
            _ => None,
        }
    }

    /// Aliases replaced by what they name (once).
    /// Whether a value of type `c` fits where `p` is declared, in LuaLS's order
    /// (`vm.isSubType`, default settings): `None` when it cannot be told here
    /// (a type declared in another file, a generic, an alias, `fun` shapes).
    fn cast(&self, c: &T, p: &T, depth: usize) -> Option<bool> {
        if depth > 8 {
            return None;
        }
        // `---@alias`, `---@enum`: their members.
        if let T::Name(n) = c
            && let Some(a) = self.aliases.get(n)
        {
            return self.cast(a, p, depth + 1);
        }
        if let T::Name(n) = p
            && let Some(a) = self.aliases.get(n)
        {
            if matches!(a, T::Generic(g, _) if g == OPAQUE_ENUM) {
                return None;
            }
            return self.cast(c, a, depth + 1);
        }
        // Every member of the value must fit.
        let all = |parts: &[T]| {
            let mut unsure = false;
            for m in parts {
                match self.cast(m, p, depth + 1) {
                    Some(false) => return Some(false),
                    None => unsure = true,
                    Some(true) => {}
                }
            }
            if unsure { None } else { Some(true) }
        };
        // A value the inference could not fully tell.
        if depth == 0 && has_unknown(c) {
            return None;
        }
        // nil is not checked (LuaLS's `Lua.type.weakNilCheck`): the code may
        // test for it where the inference does not see.
        match c {
            T::Union(ms) => return all(ms),
            T::Optional(inner) => return self.cast(inner, p, depth + 1),
            _ => {}
        }
        if is(c, "nil") || is(c, "any") {
            return Some(true);
        }
        // What the inference could not tell.
        if is(c, "unknown") {
            return None;
        }
        // One member of the declaration is enough.
        let any = |parts: &[T]| {
            let mut unsure = false;
            for m in parts {
                match self.cast(c, m, depth + 1) {
                    Some(true) => return Some(true),
                    None => unsure = true,
                    Some(false) => {}
                }
            }
            if unsure { None } else { Some(false) }
        };
        match p {
            T::Union(ms) => return any(ms),
            T::Optional(inner) => return any(&[(**inner).clone(), name("nil")]),
            _ => {}
        }
        if is(p, "any") || is(p, "unknown") {
            return Some(true);
        }
        match (c, p) {
            (T::Str(a), T::Str(b)) => return Some(a == b),
            (T::Int(a), T::Int(b)) => return Some(a == b),
            (T::Bool(a), T::Bool(b)) => return Some(a == b),
            _ => {}
        }
        if let (T::Generic(a, x), T::Generic(b, y)) = (c, p)
            && a == "table"
            && b == "table"
            && x.len() == 2
            && y.len() == 2
        {
            let mut unsure = false;
            for (m, w) in x.iter().zip(y) {
                match self.cast(m, w, depth + 1) {
                    Some(false) => return Some(false),
                    None => unsure = true,
                    Some(true) => {}
                }
            }
            return if unsure { None } else { Some(true) };
        }
        // `{ x: number }` where `{ x: number, y: number }` is declared: every
        // field the declaration needs must be there and fit.
        if let (T::Table(have), T::Table(want)) = (c, p) {
            let mut unsure = false;
            for w in want.iter().filter(|w| !w.optional && !maybe_nil(&w.ty)) {
                // `[1]` is one of `[number]`; a name is one of `[string]`.
                let same = |h: &crate::luacats::TableField| match (&h.key, &w.key) {
                    (a, b) if a == b => true,
                    (Key::Index(a), Key::Index(b)) => numeric(a).is_some() && numeric(b).is_some(),
                    (Key::Name(_), Key::Index(b)) => is(b, "string"),
                    _ => false,
                };
                let Some(h) = have.iter().find(|h| same(h)) else { return Some(false) };
                match self.cast(&h.ty, &w.ty, depth + 1) {
                    Some(false) => return Some(false),
                    None => unsure = true,
                    Some(true) => {}
                }
            }
            return if unsure { None } else { Some(true) };
        }
        let (ck, pk) = (self.cast_kind(c), self.cast_kind(p));
        // A literal declaration (`'a'|'b'`) takes a value of its base type only
        // when the value's literal is known.
        if matches!(p, T::Str(_) | T::Int(_) | T::Bool(_)) {
            return match (&ck, &pk) {
                (Kind::Basic(a), Kind::Basic(b)) if a == b || matches!((*a, *b), ("integer", "number") | ("number", "integer")) => None,
                (Kind::Basic(_), Kind::Basic(_)) => Some(false),
                _ => None,
            };
        }
        match (ck, pk) {
            (Kind::Unknown, _) | (_, Kind::Unknown) => None,
            // A table with `__call` is callable.
            (Kind::Basic("table"), Kind::Basic("function")) => None,
            (Kind::Basic(a), Kind::Basic(b)) => Some(a == b || matches!((a, b), ("integer", "number") | ("number", "integer"))),
            (Kind::Class(_), Kind::Basic("table")) | (Kind::Basic("table"), Kind::Class(_)) => Some(true),
            (Kind::Class(x), Kind::Class(y)) => {
                if x == y {
                    return Some(true);
                }
                self.class_extends(&x, &|n| n == y, 0)
            }
            (Kind::Class(x), Kind::Basic(b)) => self.class_extends(&x, &|n| n == b, 0),
            // `---@class A: string` takes a string.
            (Kind::Basic(a), Kind::Class(y)) => self.class_extends(&y, &|n| n == a, 0),
        }
    }

    /// Whether a class of this file has a parent `want` (at any depth): `None`
    /// when a parent is not a class of this file or a basic type.
    fn class_extends(&self, class: &str, want: &dyn Fn(&str) -> bool, depth: usize) -> Option<bool> {
        if depth > 8 {
            return None;
        }
        let def = self.classes.get(class)?;
        let mut unsure = false;
        for parent in &def.parents {
            if want(parent) {
                return Some(true);
            }
            if BASIC.contains(&parent.as_str()) {
                continue;
            }
            match self.class_extends(parent, want, depth + 1) {
                Some(true) => return Some(true),
                None => unsure = true,
                Some(false) => {}
            }
        }
        if unsure { None } else { Some(false) }
    }

    fn cast_kind(&self, t: &T) -> Kind {
        match t {
            T::Name(n) => match BASIC.iter().find(|b| **b == n) {
                Some(b) => Kind::Basic(b),
                None if self.classes.contains_key(n) && !self.aliases.contains_key(n) => Kind::Class(n.clone()),
                None => Kind::Unknown,
            },
            T::Str(_) => Kind::Basic("string"),
            T::Int(_) => Kind::Basic("integer"),
            T::Bool(_) => Kind::Basic("boolean"),
            T::Table(_) | T::Array(_) | T::Tuple(_) => Kind::Basic("table"),
            T::Generic(n, _) if n == "table" => Kind::Basic("table"),
            T::Fun(_) => Kind::Basic("function"),
            _ => Kind::Unknown,
        }
    }

    /// Whether a type names a literal (`1`, `'a'`, `true`) among its members,
    /// through aliases and enums.
    fn wants_literal(&self, t: &T, depth: usize) -> bool {
        match t {
            T::Str(_) | T::Int(_) | T::Bool(_) => true,
            T::Optional(i) => self.wants_literal(i, depth),
            T::Union(ps) => ps.iter().any(|p| self.wants_literal(p, depth)),
            T::Name(n) if depth < 8 => self.aliases.get(n).is_some_and(|a| self.wants_literal(a, depth + 1)),
            _ => false,
        }
    }

    fn expand(&self, t: &T) -> T {
        match t {
            T::Name(n) => self.aliases.get(n).cloned().unwrap_or_else(|| t.clone()),
            T::Optional(inner) => self.expand(inner),
            _ => t.clone(),
        }
    }

    fn global_type(&self, global: &str) -> T {
        if let Some(known) = self.globals.borrow().get(global) {
            return known.clone().unwrap_or_else(unknown);
        }
        self.globals.borrow_mut().insert(global.to_string(), None);
        let t = self.global_type_inner(global);
        self.globals.borrow_mut().insert(global.to_string(), Some(t.clone()));
        t
    }

    fn global_type_inner(&self, global: &str) -> T {
        self.deeper(|| {
            let mut parts = Vec::new();
            for r in self.names.refs.iter().filter(|r| r.target == Target::Global && r.access == Access::Write && &self.text[r.range] == global) {
                let Some(e) = self.root.token_at_offset(r.range.start()).right_biased().and_then(|t| t.parent()) else { continue };
                if let Some(v) = self.assigned_value(&e) {
                    parts.push(v);
                }
            }
            // `rawset(_G, 'Name', value)`.
            for call in self.root.descendants().filter(|n| n.kind() == CALL_EXPR) {
                let Some(callee) = call.first_child() else { continue };
                if &self.text[callee.text_range()] != "rawset" {
                    continue;
                }
                let args: Vec<SyntaxNode> = call.children().find(|c| c.kind() == ARG_LIST).map(|a| a.children().collect()).unwrap_or_default();
                if args.len() == 3
                    && &self.text[args[0].text_range()] == "_G"
                    && crate::util::unquote(&self.text[args[1].text_range()]) == Some(global)
                {
                    let tags = call.ancestors().find(|a| a.kind() == CALL_STAT).map(|s| stat_tags(self.text, &s)).unwrap_or_default();
                    match tags.into_iter().find_map(|t| match t {
                        Tag::Class { name: c, .. } => Some(c),
                        _ => None,
                    }) {
                        Some(c) => parts.push(name(&c)),
                        None => parts.push(self.expr_type(&args[2])),
                    }
                }
            }
            if parts.is_empty() {
                // The library's declared type for a global it defines.
                if let Some(item) = crate::library::library().get(global).iter().find(|i| i.function) {
                    let _ = item;
                    return name("function");
                }
                return unknown();
            }
            union(parts)
        })
    }

    fn expr_type(&self, e: &SyntaxNode) -> T {
        let key = (e.text_range(), e.kind());
        if let Some(known) = self.exprs.borrow().get(&key) {
            return known.clone().unwrap_or_else(unknown);
        }
        self.exprs.borrow_mut().insert(key, None);
        let t = match self.cast_after(e) {
            Some(t) => t,
            None => self.deeper(|| self.expr_type_inner(e)),
        };
        self.exprs.borrow_mut().insert(key, Some(t.clone()));
        t
    }

    /// `expr --[[@as T]]`.
    fn cast_after(&self, e: &SyntaxNode) -> Option<T> {
        let mut t = e.last_token()?.next_token();
        while let Some(tok) = &t {
            if tok.kind() != WHITESPACE {
                break;
            }
            t = tok.next_token();
        }
        let tok = t.filter(|t| t.kind() == BLOCK_COMMENT)?;
        let inner = tok.text().strip_prefix("--[[")?.strip_suffix("]]")?.trim().strip_prefix("@as")?;
        let (tags, _) = crate::luacats::parse_block(&format!("---@type {}", inner.trim()), TextSize::from(0));
        tags.into_iter().find_map(|t| match t.tag {
            Tag::Type { tys } => tys.into_iter().next(),
            _ => None,
        })
    }

    fn expr_type_inner(&self, e: &SyntaxNode) -> T {
        match e.kind() {
            LITERAL_EXPR => e.first_token().and_then(|t| token_kind_type(&t)).unwrap_or_else(unknown),
            TABLE_EXPR => name("table"),
            FUNCTION_EXPR => name("function"),
            VARARG_EXPR => self.vararg_type(e),
            PAREN_EXPR => e.first_child().map_or_else(unknown, |c| self.expr_type(&c)),
            NAME_EXPR => {
                let Some(tok) = e.first_token() else { return unknown() };
                match self.ref_at(tok.text_range()).map(|r| r.target) {
                    Some(Target::Decl(d)) => self.read_type(e, d),
                    Some(Target::Global) => self.global_type(tok.text()),
                    None => unknown(),
                }
            }
            UNARY_EXPR => {
                let op = e.first_token().map(|t| t.kind());
                let operand = e.first_child().map(|c| self.expr_type(&c));
                let op_name = match op {
                    Some(MINUS) => "unm",
                    Some(HASH) => "len",
                    Some(TILDE) => "bnot",
                    _ => "",
                };
                if let Some(t) = operand.as_ref().and_then(|o| self.operator(o, op_name, None)) {
                    return t;
                }
                match op {
                    Some(NOT_KW) => name("boolean"),
                    Some(HASH) => name("integer"),
                    Some(TILDE) => name("integer"),
                    Some(MINUS) => match operand.as_ref().and_then(numeric) {
                        Some(true) => name("integer"),
                        _ => name("number"),
                    },
                    _ => unknown(),
                }
            }
            BINARY_EXPR => self.binary_type(e),
            CALL_EXPR | METHOD_CALL_EXPR => self.call_returns(e).into_iter().next().unwrap_or_else(|| name("nil")),
            INDEX_EXPR => {
                let Some(object) = e.first_child() else { return unknown() };
                let key = match crate::lint::field_name(e) {
                    Some(f) => Key::Name(f.text().to_string()),
                    None => match e.children().nth(1) {
                        Some(k) if k.kind() == LITERAL_EXPR => {
                            let s = &self.text[k.text_range()];
                            match crate::util::unquote(s) {
                                Some(v) => Key::Name(v.to_string()),
                                None => Key::Index(s.parse::<i64>().map(T::Int).unwrap_or_else(|_| self.expr_type(&k))),
                            }
                        }
                        Some(k) => Key::Index(self.expr_type(&k)),
                        None => return unknown(),
                    },
                };
                self.index_type(&object, &key)
            }
            _ => unknown(),
        }
    }

    /// The class (or one of its parents) declaring `---@operator op`.
    fn class_def(&self, c: &str) -> Option<&crate::api::ClassDef> {
        self.classes.get(c).or_else(|| crate::library::classes().get(c).and_then(|v| v.first()))
    }

    /// `---@operator` of the operand's class: its result.
    fn operator(&self, t: &T, op: &str, other: Option<&T>) -> Option<T> {
        let T::Name(c) = self.expand(t) else { return None };
        let mut todo = vec![c];
        let mut seen = std::collections::HashSet::new();
        while let Some(c) = todo.pop() {
            if !seen.insert(c.clone()) || seen.len() > 32 {
                continue;
            }
            let Some(def) = self.class_def(&c) else { continue };
            let ops: Vec<&(String, Option<T>, T)> = def.operators.iter().filter(|(o, _, _)| o == op).collect();
            // The overload whose operand fits, else the first.
            let fits = |p: &Option<T>| match (p, other) {
                (Some(p), Some(o)) => {
                    let parts: Vec<&T> = match p {
                        T::Union(v) => v.iter().collect(),
                        p => vec![p],
                    };
                    parts.iter().any(|p| **p == *o || (numeric(p).is_some() && numeric(o).is_some()))
                }
                _ => true,
            };
            if let Some((_, _, r)) = ops.iter().find(|(_, p, _)| fits(p)).or(ops.first()) {
                return Some(r.clone());
            }
            todo.extend(def.parents.iter().cloned());
        }
        None
    }

    fn binary_type(&self, e: &SyntaxNode) -> T {
        let mut kids = e.children();
        let (Some(l), Some(r)) = (kids.next(), kids.next()) else { return unknown() };
        let Some(op) = e.children_with_tokens().filter_map(|x| x.into_token()).find(|t| !matches!(t.kind(), WHITESPACE | COMMENT)) else {
            return unknown();
        };
        let op_name = match op.kind() {
            PLUS => Some("add"),
            MINUS => Some("sub"),
            STAR => Some("mul"),
            SLASH => Some("div"),
            PERCENT => Some("mod"),
            CARET => Some("pow"),
            SLASH2 => Some("idiv"),
            AMP => Some("band"),
            PIPE => Some("bor"),
            TILDE => Some("bxor"),
            SHL => Some("shl"),
            SHR => Some("shr"),
            DOT2 => Some("concat"),
            _ => None,
        };
        if let Some(name) = op_name {
            let (lt, rt) = (self.expr_type(&l), self.expr_type(&r));
            if let Some(t) = self.operator(&lt, name, Some(&rt)).or_else(|| self.operator(&rt, name, Some(&lt))) {
                return t;
            }
        }
        match op.kind() {
            EQ2 | NEQ | LT | GT | LTEQ | GTEQ => name("boolean"),
            // `a and b`: `b`, or what of `a` is falsy (`nil`, `false`).
            AND_KW => {
                let lt = self.bool_const(&l).map_or_else(|| self.expr_type(&l), T::Bool);
                if lt == T::Bool(true) {
                    return self.expr_type(&r);
                }
                if is(&lt, "nil") {
                    return lt;
                }
                let rt = self.expr_type(&r);
                if has_unknown(&lt) || mentions_any(&lt) {
                    return rt;
                }
                let falsy: Vec<T> = members_of(&lt)
                    .into_iter()
                    .filter_map(|m| match m {
                        T::Name(n) if n == "nil" => Some(name("nil")),
                        T::Name(n) if n == "boolean" => Some(T::Bool(false)),
                        T::Bool(false) => Some(T::Bool(false)),
                        _ => None,
                    })
                    .collect();
                bool_union(std::iter::once(rt).chain(falsy).collect())
            }
            // `a or b`: what of `a` is truthy, and `b` when `a` may be falsy.
            OR_KW => {
                let lt = self.bool_const(&l).map_or_else(|| self.expr_type(&l), T::Bool);
                let rt = self.expr_type(&r);
                if lt == T::Bool(false) {
                    return rt;
                }
                if matches!(&lt, T::Name(n) if n == "nil" || n == "boolean" && self.text[l.text_range()].trim() == "false") {
                    return rt;
                }
                let ms = members_of(&lt);
                let falsy = |m: &T| is(m, "nil") || matches!(m, T::Bool(false));
                if has_unknown(&lt) || !ms.iter().any(|m| falsy(m) || is(m, "boolean")) {
                    return lt;
                }
                bool_union(ms.into_iter().filter(|m| !falsy(m)).chain(std::iter::once(rt)).collect())
            }
            DOT2 => {
                let ok = |t: &T| is(t, "string") || numeric(t).is_some();
                if ok(&self.expr_type(&l)) && ok(&self.expr_type(&r)) { name("string") } else { unknown() }
            }
            PLUS | MINUS | STAR | PERCENT | SLASH2 => match (numeric(&self.expr_type(&l)), numeric(&self.expr_type(&r))) {
                (Some(true), Some(true)) => name("integer"),
                (Some(_), Some(_)) => name("number"),
                _ => unknown(),
            },
            SLASH | CARET => match (numeric(&self.expr_type(&l)), numeric(&self.expr_type(&r))) {
                (Some(_), Some(_)) => name("number"),
                _ => unknown(),
            },
            AMP | PIPE | TILDE | SHL | SHR => match (numeric(&self.expr_type(&l)), numeric(&self.expr_type(&r))) {
                (Some(_), Some(_)) => name("integer"),
                _ => unknown(),
            },
            _ => unknown(),
        }
    }

    /// What `object[key]` / `object.key` holds.
    fn index_type(&self, object: &SyntaxNode, key: &Key) -> T {
        self.deeper(|| {
            let ot = self.expr_type(object);
            if let Some(t) = self.member_of_type(&ot, key) {
                return t;
            }
            // A table constructor's field.
            if let Some(ctor) = self.constructor(object) {
                let mut pos = 0i64;
                for f in ctor.children() {
                    let value = f.children().last();
                    let hit = match (f.kind(), key) {
                        (FIELD_NAMED, Key::Name(k)) => {
                            f.children_with_tokens().filter_map(|x| x.into_token()).find(|t| t.kind() == NAME).is_some_and(|t| t.text() == k)
                        }
                        (FIELD_POS, Key::Index(T::Int(i))) => {
                            pos += 1;
                            pos == *i
                        }
                        (FIELD_INDEXED, Key::Name(k)) => {
                            f.first_child().and_then(|k2| crate::util::unquote(&self.text[k2.text_range()]).map(str::to_string)).as_deref() == Some(k)
                        }
                        _ => false,
                    };
                    if hit && let Some(v) = value {
                        return self.expr_type(&v);
                    }
                }
            }
            unknown()
        })
    }

    /// A field of a type: class `---@field`s (parents too), `table<K, V>`,
    /// `T[]`, `{ k: T }`.
    fn member_of_type(&self, t: &T, key: &Key) -> Option<T> {
        // Aliases may name themselves (`---@alias A A|nil`).
        let d = self.depth.get();
        if d >= MAX_DEPTH {
            return None;
        }
        self.depth.set(d + 1);
        let found = self.member_of_type_inner(t, key);
        self.depth.set(d);
        found
    }

    fn member_of_type_inner(&self, t: &T, key: &Key) -> Option<T> {
        match t {
            T::Optional(inner) => self.member_of_type(inner, key),
            T::Union(parts) => {
                let found: Vec<T> = parts.iter().filter_map(|p| self.member_of_type(p, key)).collect();
                (!found.is_empty()).then(|| union(found))
            }
            T::Array(inner) => matches!(key, Key::Index(_)).then(|| (**inner).clone()),
            T::Generic(n, args) if n == "table" && args.len() == 2 => Some(args[1].clone()),
            T::Table(fields) => fields
                .iter()
                .find(|f| &f.key == key)
                .or_else(|| {
                    let Key::Index(k) = key else { return None };
                    fields.iter().find(|f| matches!(&f.key, Key::Index(w) if numeric(w).is_some() && numeric(k).is_some()))
                })
                .map(|f| f.ty.clone()),
            T::Generic(n, args) if self.class_def(n).is_some_and(|c| !c.generics.is_empty()) => {
                let c = self.class_def(n)?;
                let map: HashMap<String, T> = c.generics.iter().cloned().zip(args.iter().cloned()).collect();
                let t = self.member_of_type(&name(n), key)?;
                Some(subst(&t, &map, &c.generics, true))
            }
            T::Name(n) => {
                if let Some(a) = self.aliases.get(n) {
                    return self.member_of_type(&a.clone(), key);
                }
                let k = match key {
                    Key::Name(k) => k.clone(),
                    Key::Index(kt) => {
                        let def = self.class_def(n)?;
                        let fits = |want: &T| match (want, kt) {
                            (w, k) if w == k => true,
                            (w, k) => numeric(w).is_some() && numeric(k).is_some() || is(w, "any"),
                        };
                        return def.indexes.iter().find(|(want, _)| fits(want)).map(|(_, v)| v.clone());
                    }
                };
                let k = &k;
                let mut todo = vec![n.clone()];
                let mut seen = std::collections::HashSet::new();
                while let Some(c) = todo.pop() {
                    if !seen.insert(c.clone()) || seen.len() > 32 {
                        continue;
                    }
                    let lib;
                    let def = match self.classes.get(&c) {
                        Some(d) => d,
                        None => {
                            lib = crate::library::classes().get(&c).and_then(|v| v.first());
                            match lib {
                                Some(d) => d,
                                None => continue,
                            }
                        }
                    };
                    if let Some(f) = def.fields.iter().find(|f| &f.name == k) {
                        return Some(f.ty.clone());
                    }
                    todo.extend(def.parents.iter().cloned());
                }
                None
            }
            _ => None,
        }
    }

    /// Every value a call returns.
    fn call_returns(&self, call: &SyntaxNode) -> Vec<T> {
        let Some(callee) = call.first_child() else { return Vec::new() };
        let args: Vec<SyntaxNode> = call.children().find(|c| c.kind() == ARG_LIST).map(|a| a.children().collect()).unwrap_or_default();
        if call.kind() == METHOD_CALL_EXPR {
            let Some(m) = crate::api::method_name(call) else { return vec![unknown()] };
            let object = self.expr_type(&callee);
            let info = self.method_info(&callee, m.text()).or_else(|| {
                let ft = self.index_type(&callee, &Key::Name(m.text().to_string()));
                fun_info(&ft)
            });
            let Some(mut info) = info else { return vec![unknown()] };
            // `function C.new(self)` called as `C:new()`: `self` is the object, and
            // a `---@generic T` / `---@param self T` binds to its type.
            if callee.kind() == NAME_EXPR
                && let Some(body) = self.file_functions().get(&format!("{}.{}", &self.text[callee.text_range()], m.text()))
                && body.parent().and_then(|s| s.children().find(|c| c.kind() == FUNC_NAME)).is_some_and(|f| !f.children_with_tokens().any(|e| e.kind() == COLON))
                && body
                    .children()
                    .find(|c| c.kind() == PARAM_LIST)
                    .and_then(|l| l.children().find(|c| c.kind() == PARAM))
                    .and_then(|p| p.first_token())
                    .is_some_and(|t| t.text() == "self")
                && !info.params.is_empty()
            {
                let first = info.params.remove(0);
                if let Some(T::Name(g)) = first
                    && info.generics.contains(&g)
                {
                    let map: HashMap<String, T> = [(g.clone(), object.clone())].into();
                    let only = [g.clone()];
                    info.rets = info.rets.iter().map(|r| subst(r, &map, &only, true)).collect();
                    info.generics.retain(|x| *x != g);
                }
            }
            return self.apply(&info, &args).into_iter().map(|t| if is(&t, "self") { object.clone() } else { t }).collect();
        }
        let callee_name = (callee.kind() == NAME_EXPR).then(|| self.text[callee.text_range()].to_string());
        let global = callee.first_token().and_then(|t| self.ref_at(t.text_range())).is_some_and(|r| r.target == Target::Global);
        if global && let Some(n) = callee_name.as_deref() {
            match n {
                "setmetatable" => return vec![args.first().map_or_else(unknown, |a| self.expr_type(a))],
                "pcall" | "xpcall" => {
                    let Some(f) = args.first() else { return vec![name("boolean")] };
                    let mut out = vec![name("boolean")];
                    let inner = self.callable_returns(f).unwrap_or_else(|| vec![name("any")]);
                    out.extend(if inner.is_empty() { vec![name("any")] } else { inner });
                    return out;
                }
                _ => {}
            }
        }
        if let Some(rets) = self.overload_returns(&callee, &args) {
            return rets;
        }
        if let Some(info) = self.callee_info(&callee) {
            return self.apply(&info, &args);
        }
        // `---@operator call: A`.
        if let Some(t) = self.operator(&self.expr_type(&callee), "call", None) {
            return vec![t];
        }
        if global
            && let Some(n) = callee_name
            && let Some(item) = crate::library::library().get(&n).iter().find(|i| i.function)
        {
            return crate::library::returns(item);
        }
        vec![unknown()]
    }

    /// What calling the value of `f` returns, arguments not considered.
    fn callable_returns(&self, f: &SyntaxNode) -> Option<Vec<T>> {
        self.callee_info(f).map(|i| i.rets)
    }

    /// A callee's parameter types, returns and generic names: a function of the
    /// file (its `---@param` / `---@return`, else its `return` statements) or a
    /// `fun(...)` type.
    fn callee_info(&self, f: &SyntaxNode) -> Option<Callee> {
        if let Some(body) = self.function_body(f) {
            return Some(self.body_info(&body));
        }
        let mut info = fun_info(&self.expr_type(f))?;
        // `---@generic K` on the local holding the `fun` type.
        if f.kind() == NAME_EXPR
            && let Some(Target::Decl(d)) = f.first_token().and_then(|t| self.ref_at(t.text_range())).map(|r| r.target)
            && let Some(stat) = self.decl_stat(d)
        {
            info.generics = generics(&stat_tags(self.text, &stat));
        }
        Some(info)
    }

    /// `obj:m()` where the file has `function obj:m()` / `function obj.m()`.
    fn method_info(&self, object: &SyntaxNode, m: &str) -> Option<Callee> {
        let body = self.method_body(object, m)?;
        Some(self.body_info(&body))
    }

    /// The body of `function O.m()` / `function O:m()` of this file, for `O:m()`.
    fn method_body(&self, object: &SyntaxNode, m: &str) -> Option<SyntaxNode> {
        if object.kind() != NAME_EXPR {
            return None;
        }
        let obj = &self.text[object.text_range()];
        self.file_functions().get(&format!("{obj}.{m}")).cloned()
    }

    /// `function C.f()` (not `function C:f()`).
    fn dot_defined(&self, body: &SyntaxNode) -> bool {
        body.parent()
            .filter(|s| s.kind() == FUNCTION_STAT)
            .and_then(|s| s.children().find(|c| c.kind() == FUNC_NAME))
            .is_some_and(|f| !f.children_with_tokens().any(|e| e.kind() == COLON))
    }

    /// A name of a local that a `---@type` / `---@class` declares and nothing
    /// writes again: its type does not depend on the flow.
    fn fixed_doc_local(&self, e: &SyntaxNode) -> bool {
        if e.kind() != NAME_EXPR {
            return false;
        }
        let Some(Target::Decl(d)) = e.first_token().and_then(|t| self.ref_at(t.text_range())).map(|r| r.target) else { return false };
        let written = self.written.get_or_init(|| {
            self.names
                .refs
                .iter()
                .filter(|r| matches!(r.access, Access::Write | Access::ReadWrite))
                .filter_map(|r| match r.target {
                    Target::Decl(d) => Some(d),
                    Target::Global => None,
                })
                .collect()
        });
        self.names.decls[d].kind == DeclKind::Local
            && !written.contains(&d)
            && self.decl_stat(d).is_some_and(|st| stat_tags(self.text, &st).iter().any(|t| matches!(t, Tag::Type { .. } | Tag::Class { .. })))
    }

    fn file_functions(&self) -> &HashMap<String, SyntaxNode> {
        self.functions.get_or_init(|| {
            let mut out = HashMap::new();
            for s in self.root.descendants().filter(|s| s.kind() == FUNCTION_STAT) {
                let (Some(fname), Some(body)) = (s.children().find(|c| c.kind() == FUNC_NAME), s.children().find(|c| c.kind() == FUNC_BODY)) else { continue };
                let key: String = self.text[fname.text_range()].split_whitespace().collect::<String>().replace(':', ".");
                out.entry(key).or_insert(body);
            }
            out
        })
    }

    /// The statement declaring a local.
    /// `true` / `false`, or a local given one of them and never written again.
    fn bool_const(&self, e: &SyntaxNode) -> Option<bool> {
        let lit = |v: &SyntaxNode| match v.first_token().filter(|_| v.kind() == LITERAL_EXPR).map(|t| t.kind()) {
            Some(TRUE_KW) => Some(true),
            Some(FALSE_KW) => Some(false),
            _ => None,
        };
        match e.kind() {
            LITERAL_EXPR => lit(e),
            PAREN_EXPR => self.bool_const(&e.first_child()?),
            NAME_EXPR => {
                let Target::Decl(d) = self.ref_at(e.first_token()?.text_range())?.target else { return None };
                if self.names.refs.iter().any(|r| r.target == Target::Decl(d) && r.access == Access::Write) {
                    return None;
                }
                let stat = self.decl_stat(d).filter(|s| s.kind() == LOCAL_STAT)?;
                let range = self.names.decls[d].range;
                let i = stat.children().filter(|c| c.kind() == LOCAL_NAME).position(|n| n.text_range().contains_range(range))?;
                lit(&stat.children().find(|c| c.kind() == EXPR_LIST)?.children().nth(i)?)
            }
            _ => None,
        }
    }

    fn decl_stat(&self, d: usize) -> Option<SyntaxNode> {
        // Finding the token walks the tree from the top, linear in a long block.
        if let Some(known) = self.decl_stats.borrow().get(&d) {
            return known.clone();
        }
        let decl = &self.names.decls[d];
        let tok = self.root.token_at_offset(decl.range.start()).right_biased();
        let st = tok.and_then(|t| t.parent_ancestors().find(|a| matches!(a.kind(), LOCAL_STAT | LOCAL_FUNCTION_STAT)));
        self.decl_stats.borrow_mut().insert(d, st.clone());
        st
    }

    fn body_info(&self, body: &SyntaxNode) -> Callee {
        let holder = body.parent();
        let stat = holder.as_ref().and_then(|h| match h.kind() {
            FUNCTION_STAT | LOCAL_FUNCTION_STAT => Some(h.clone()),
            FUNCTION_EXPR => h.ancestors().find(|a| matches!(a.kind(), LOCAL_STAT | ASSIGN_STAT)),
            _ => None,
        });
        let tags = stat.as_ref().map(|s| stat_tags(self.text, s)).unwrap_or_default();
        let params: Vec<Option<T>> = body
            .children()
            .find(|c| c.kind() == PARAM_LIST)
            .map(|l| {
                l.children()
                    .filter(|p| p.kind() == PARAM)
                    .map(|p| {
                        let n = p.first_token().map(|t| t.text().to_string()).unwrap_or_default();
                        tags.iter().find_map(|t| match t {
                            Tag::Param { name: pn, ty: Some(ty), .. } if *pn == n => Some(ty.clone()),
                            _ => None,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Callee { params, rets: self.body_returns(body), generics: generics(&tags) }
    }

    /// The returns of a call with these arguments: generics bound from the
    /// arguments' types.
    fn apply(&self, info: &Callee, args: &[SyntaxNode]) -> Vec<T> {
        self.bound_returns(info, args, true)
    }

    /// The returns with the generics the arguments bind; `shown` writes the
    /// unbound ones as LuaLS shows them, else they keep their names for a later
    /// binding (an iterator's state).
    fn bound_returns(&self, info: &Callee, args: &[SyntaxNode], shown: bool) -> Vec<T> {
        if info.generics.is_empty() {
            return info.rets.clone();
        }
        let mut map = HashMap::new();
        for (i, a) in args.iter().enumerate() {
            if let Some(Some(p)) = info.params.get(i) {
                bind(p, &self.expr_type(a), &info.generics, &mut map);
            }
        }
        if !shown {
            let open: Vec<String> = info.generics.iter().filter(|g| map.contains_key(*g)).cloned().collect();
            return info.rets.iter().map(|t| subst(t, &map, &open, true)).collect();
        }
        info.rets.iter().map(|t| subst(t, &map, &info.generics, true)).collect()
    }

    /// The function body a name or expression refers to, in this file.
    fn function_body(&self, e: &SyntaxNode) -> Option<SyntaxNode> {
        match e.kind() {
            FUNCTION_EXPR => e.children().find(|c| c.kind() == FUNC_BODY),
            PAREN_EXPR => self.function_body(&e.first_child()?),
            NAME_EXPR => {
                let tok = e.first_token()?;
                match self.ref_at(tok.text_range())?.target {
                    Target::Decl(d) => {
                        let decl = &self.names.decls[d];
                        let dt = self.root.token_at_offset(decl.range.start()).right_biased()?;
                        match decl.kind {
                            DeclKind::LocalFunction => dt.parent()?.children().find(|c| c.kind() == FUNC_BODY),
                            DeclKind::Local => {
                                let f = decl.function?;
                                let fe = self.root.covering_element(f);
                                let fe = fe.as_node()?.ancestors().find(|a| a.kind() == FUNCTION_EXPR)?;
                                fe.children().find(|c| c.kind() == FUNC_BODY)
                            }
                            _ => None,
                        }
                    }
                    // `function n()` in the file.
                    Target::Global => self.file_functions().get(tok.text()).cloned(),
                }
            }
            // `C.f(...)` on `function C.f()` of this file (a `:` one takes `self` first).
            INDEX_EXPR => {
                let key: String = self.text[e.text_range()].split_whitespace().collect();
                let body = self.file_functions().get(&key)?.clone();
                self.dot_defined(&body).then_some(body)
            }
            _ => None,
        }
    }

    /// A function's `---@return` types, else the union of its `return`
    /// statements by position.
    fn body_returns(&self, body: &SyntaxNode) -> Vec<T> {
        let key = body.text_range();
        if let Some(known) = self.bodies.borrow().get(&key) {
            return known.clone().unwrap_or_else(|| vec![unknown()]);
        }
        self.bodies.borrow_mut().insert(key, None);
        let d = self.depth.get();
        let t = if d >= MAX_DEPTH {
            vec![unknown()]
        } else {
            self.depth.set(d + 1);
            let t = self.body_returns_inner(body);
            self.depth.set(d);
            t
        };
        self.bodies.borrow_mut().insert(key, Some(t.clone()));
        t
    }

    fn body_returns_inner(&self, body: &SyntaxNode) -> Vec<T> {
        let holder = body.parent();
        let stat = holder.as_ref().and_then(|h| match h.kind() {
            FUNCTION_STAT | LOCAL_FUNCTION_STAT => Some(h.clone()),
            FUNCTION_EXPR => h.ancestors().find(|a| matches!(a.kind(), LOCAL_STAT | ASSIGN_STAT)),
            _ => None,
        });
        if let Some(stat) = stat {
            let rets: Vec<T> = stat_tags(self.text, &stat)
                .into_iter()
                .filter_map(|t| match t {
                    Tag::Return { rets, .. } => Some(rets),
                    _ => None,
                })
                .flatten()
                .map(|r| if matches!(r.ty, T::Dots) { unknown() } else { r.ty })
                .collect();
            if !rets.is_empty() {
                return rets;
            }
        }
        let mut cols: Vec<Vec<T>> = Vec::new();
        let mut returns = 0;
        for r in body.descendants().filter(|n| n.kind() == RETURN_STAT) {
            // Only the function's own returns.
            if r.ancestors().find(|a| a.kind() == FUNC_BODY).as_ref() != Some(body) {
                continue;
            }
            returns += 1;
            let values: Vec<SyntaxNode> = r.children().find(|c| c.kind() == EXPR_LIST).map(|l| l.children().collect()).unwrap_or_default();
            let n = match values.last().map(|v| v.kind()) {
                Some(CALL_EXPR | METHOD_CALL_EXPR) => values.len() - 1 + self.call_returns(values.last().unwrap()).len().max(1),
                _ => values.len(),
            };
            for i in 0..n.max(cols.len()) {
                let t = if i < n { self.value_at(&values, i) } else { name("nil") };
                if cols.len() <= i {
                    // Earlier returns gave nothing here.
                    cols.push(if returns > 1 { vec![name("nil")] } else { Vec::new() });
                }
                cols[i].push(t);
            }
        }
        cols.into_iter().map(union).collect()
    }
}

fn fun_rets(f: &FunType) -> Vec<T> {
    f.rets.iter().map(|r| if matches!(r.ty, T::Dots) { unknown() } else { r.ty.clone() }).collect()
}

/// Type names in `t` that are neither builtin nor a known class or alias:
/// generics of a `fun` type without `---@generic`.
fn open_names(t: &T, out: &mut Vec<String>, inf: &Infer) {
    match t {
        T::Name(n) => {
            let builtin = matches!(n.as_str(), "nil" | "any" | "unknown" | "boolean" | "number" | "integer" | "string" | "table" | "function" | "thread" | "userdata");
            if !builtin && inf.class_def(n).is_none() && !inf.aliases.contains_key(n) && !out.contains(n) {
                out.push(n.clone());
            }
        }
        T::Optional(i) | T::Array(i) => open_names(i, out, inf),
        T::Union(ps) => ps.iter().for_each(|p| open_names(p, out, inf)),
        T::Generic(_, args) => args.iter().for_each(|a| open_names(a, out, inf)),
        _ => {}
    }
}

/// What a call needs to know about the function called.
/// A class with its parents, and the `---@field` names they declare.
type ClassChain = (Vec<String>, std::collections::HashSet<String>);

/// `T?`, `T|nil` or `nil`.
fn maybe_nil(t: &T) -> bool {
    match t {
        T::Optional(_) => true,
        T::Name(n) => n == "nil",
        T::Union(ps) => ps.iter().any(maybe_nil),
        _ => false,
    }
}

fn mentions_any(t: &T) -> bool {
    match t {
        T::Name(n) => n == "any",
        T::Union(ps) => ps.iter().any(mentions_any),
        T::Optional(i) => mentions_any(i),
        _ => false,
    }
}

/// A field a table may leave out: optional, nil-able, `any` or `unknown`.
fn nil_able(t: &T) -> bool {
    match t {
        T::Optional(_) => true,
        T::Name(n) => matches!(n.as_str(), "nil" | "any" | "unknown"),
        T::Union(ps) => ps.iter().any(nil_able),
        _ => false,
    }
}

fn has_unknown(t: &T) -> bool {
    match t {
        T::Name(n) => n == "unknown",
        T::Union(ps) => ps.iter().any(has_unknown),
        T::Optional(i) => has_unknown(i),
        _ => false,
    }
}

/// The types `cast` compares by name (`true` / `false` are `boolean`).
const BASIC: &[&str] = &["nil", "boolean", "number", "integer", "string", "table", "function", "thread", "userdata", "lightuserdata"];

enum Kind {
    Basic(&'static str),
    /// A `---@class` of this file.
    Class(String),
    Unknown,
}

struct Callee {
    params: Vec<Option<T>>,
    rets: Vec<T>,
    generics: Vec<String>,
}

fn fun_info(t: &T) -> Option<Callee> {
    match t {
        T::Fun(f) => Some(Callee { params: f.params.iter().map(|p| p.ty.clone()).collect(), rets: fun_rets(f), generics: Vec::new() }),
        T::Optional(inner) => fun_info(inner),
        _ => None,
    }
}

fn generics(tags: &[Tag]) -> Vec<String> {
    tags.iter()
        .filter_map(|t| match t {
            Tag::Generic { names } => Some(names.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>()),
            _ => None,
        })
        .flatten()
        .collect()
}

/// Generic names in `pattern` take their part of `actual`.
fn bind(pattern: &T, actual: &T, gens: &[String], out: &mut HashMap<String, T>) {
    match (pattern, actual) {
        (T::Name(g), a) if gens.contains(g) => {
            if !is(a, "unknown") && !is(a, "nil") && !is(a, "any") {
                out.entry(g.clone()).or_insert_with(|| a.clone());
            }
        }
        (T::Optional(p), a) => bind(p, a, gens, out),
        (T::Union(ps), a) => {
            // `K|integer` given an integer binds nothing.
            let plain = ps.iter().any(|p| p == a || (numeric(p).is_some() && numeric(a).is_some()));
            if !plain {
                for p in ps {
                    bind(p, a, gens, out);
                }
            }
        }
        (T::Generic(pn, pa), T::Generic(an, aa)) if pn == an && pa.len() == aa.len() => {
            for (p, a) in pa.iter().zip(aa) {
                bind(p, a, gens, out);
            }
        }
        (T::Generic(pn, pa), T::Array(e)) if pn == "table" && pa.len() == 2 => {
            bind(&pa[0], &name("integer"), gens, out);
            bind(&pa[1], e, gens, out);
        }
        (T::Array(p), T::Array(a)) => bind(p, a, gens, out),
        (T::Array(p), T::Generic(an, aa)) if an == "table" && aa.len() == 2 => bind(p, &aa[1], gens, out),
        (T::Fun(pf), T::Fun(af)) => {
            for (p, a) in pf.params.iter().zip(&af.params) {
                if let (Some(p), Some(a)) = (&p.ty, &a.ty) {
                    bind(p, a, gens, out);
                }
            }
            for (p, a) in pf.rets.iter().zip(&af.rets) {
                bind(&p.ty, &a.ty, gens, out);
            }
        }
        _ => {}
    }
}

/// Bound generics replaced; an unbound one is `unknown` at the top and `<T>`
/// inside another type, as LuaLS shows them.
fn subst(t: &T, map: &HashMap<String, T>, gens: &[String], top: bool) -> T {
    let s = |x: &T| subst(x, map, gens, false);
    match t {
        T::Name(g) if gens.contains(g) => map.get(g).cloned().unwrap_or_else(|| if top { unknown() } else { name(&format!("<{g}>")) }),
        T::Optional(i) => T::Optional(Box::new(s(i))),
        T::Union(ps) => union(ps.iter().map(|p| subst(p, map, gens, top)).collect()),
        T::Array(i) => T::Array(Box::new(s(i))),
        T::Generic(n, args) => T::Generic(n.clone(), args.iter().map(s).collect()),
        T::Fun(f) => {
            let mut f = (**f).clone();
            for p in &mut f.params {
                p.ty = p.ty.as_ref().map(s);
            }
            for r in &mut f.rets {
                r.ty = s(&r.ty);
            }
            T::Fun(Box::new(f))
        }
        _ => t.clone(),
    }
}

#[cfg(test)]
mod tests {
    fn ty(t: &str) -> String {
        let o = t.find("<?").unwrap();
        let text = t.replacen("<?", "", 1).replacen("?>", "", 1);
        let p = crate::parse(&text);
        let n = crate::names(&p, &text);
        super::type_at(&p, &n, &text, crate::TextSize::from(o as u32)).unwrap_or_default()
    }

    #[test]
    fn overrides_and_overloads() {
        // A parent's `---@param`, and classes that inherit each other: bounded.
        assert_eq!(ty("---@class A\nlocal A = {}\n---@param x number\nfunction A:f(x) end\n---@class B: A\nlocal B = {}\nfunction B:f(<?x?>) end\n"), "number");
        assert_eq!(ty("---@class A: B\nlocal A = {}\n---@class B: A\nlocal B = {}\nfunction A:f(x) end\nfunction B:f(<?x?>) end\n"), "any");
        assert_eq!(ty("---@overload fun(a: 'x'): 1\n---@overload fun(a: 'y'): 2\nlocal function f(...) end\nlocal <?r?> = f('y')\n"), "2");
    }

    #[test]
    fn basics() {
        assert_eq!(ty("local <?x?> = 1"), "integer");
        assert_eq!(ty("local <?x?> = 1.5"), "number");
        assert_eq!(ty("local <?x?> = 'a' .. 1"), "string");
        assert_eq!(ty("local x\nx = 'a'\nprint(<?x?>)"), "string");
        assert_eq!(ty("---@type xPlayer\nlocal <?p?>"), "xPlayer");
        assert_eq!(ty("local function f() return 1 end\nlocal <?y?> = f()"), "integer");
        assert_eq!(ty("local t = { a = 1 }\nprint(t.<?a?>)"), "integer");
        assert_eq!(ty("for <?i?> = 1, 2 do end"), "integer");
        assert_eq!(ty("local c = GetEntityCoords(1)\nprint(c.<?x?>)"), "number");
        // Cycles end.
        assert_eq!(ty("local function f() return f() end\nlocal <?y?> = f()"), "unknown");
        let started = std::time::Instant::now();
        for t in [
            "---@alias A A|nil\n---@type A\nlocal x\nprint(x.<?y?>)",
            "---@alias A B\n---@alias B A\n---@type A\nlocal x\nlocal <?z?> = x + 1",
            "---@class C: D\n---@class D: C\n---@type C\nlocal x\nprint(x.<?y?>, -x)",
            "---@alias A {f: A}\n---@type A\nlocal x\nfor k, <?v?> in pairs(x) do end",
            "local a = b\nlocal b = a\nprint(<?a?>)",
            // Branching cycles: each level doubles without a guard.
            "---@alias A A|A\n---@type A\nlocal x\nprint(x.<?y?>, x + 1, -x)\nfor k, v in pairs(x) do end",
            "---@alias A A|B\n---@alias B A|B\n---@type A\nlocal x\nprint(x.<?y?>, x + 1, -x)\nfor k, v in pairs(x) do end",
            "---@alias A A|B\n---@alias B A|B\n---@type A\nlocal x\nlocal <?z?> = -x",
            "---@alias A A|B\n---@alias B A|B\n---@type A\nlocal x\nfor k, <?v?> in pairs(x) do end",
            "---@alias A A|B\n---@alias B A|B\n---@type A\nlocal <?x?>",
        ] {
            ty(t);
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
    }
}
