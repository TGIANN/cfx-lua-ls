//! What a file defines for other files to use: values assigned to globals,
//! locals and table fields, function signatures, the module a file returns and
//! the exports it registers. Values are kept as references (`Ref`) that a query
//! follows later across files and resources (`api_query.rs`), so a framework
//! object (`local QBCore = exports['qb-core']:GetCoreObject()`), ox_lib's `lib`
//! or `exports.res:Name` resolve against the workspace's own copy of the
//! resource at run time. Nothing from a framework is embedded.
//!
//! The summary holds no syntax nodes. Expressions are followed to a fixed depth,
//! so deep chains and nested tables stay cheap.

use crate::luacats::{DocType, Key, Tag};
use crate::names::{Names, Target};
use crate::{Parse, TextRange, TextSize};
use cfx_lua_syntax::SyntaxKind::*;
use cfx_lua_syntax::{SyntaxNode, SyntaxToken};
use std::collections::HashMap;

/// A value as far as one file can tell.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Ref {
    Unknown,
    Global(String),
    /// A local of this file, by its index in `Names::decls`.
    Local(u32),
    /// A table constructor of this file (`FileApi::tables` counts them).
    Table(u32),
    /// A function of this file, by index in `FileApi::funcs`.
    Func(u32),
    Field(Box<Ref>, String),
    /// The first value a call returns.
    Call(Box<Ref>),
    /// `require 'a.b'` / `require '@res.a.b'` (ox_lib's module loader).
    Require(String),
    /// `exports.res:Name` / `exports['res']:Name`: the exported function.
    Export(String, String),
    /// A value of a type a doc comment declares (`---@type xPlayer`,
    /// `---@return vector3`, a `---@class` local).
    Doc(DocType),
}

/// What a definition is attached to.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Owner {
    Global(String),
    Local(u32),
    Table(u32),
}

/// `owner.path... = value`; an empty path defines the owner itself (`X = v`,
/// `local x = v`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Def {
    pub owner: Owner,
    pub path: Vec<String>,
    pub value: Ref,
    /// The defined name's token.
    pub offset: u32,
    /// The comment lines above the statement, dashes removed.
    pub doc: String,
}

/// A function that registers an export under its first parameter with its
/// second as the value (`exports(name, fn)` inside, or the `__cfx_export_res_%s`
/// event); `resource`: the resource whose export it registers, when named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wrapper {
    pub resource: Option<String>,
    /// The name registered is computed from the name parameter
    /// (`exports(name:sub(1, 1):upper() .. name:sub(2), fn)`): tgiann-core's
    /// `Export` registers both first-letter casings this way.
    pub derived: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuncDef {
    /// Parameter names, `...` included; no `self`.
    pub params: Vec<String>,
    /// `function a:b()`.
    pub method: bool,
    /// The first `return` with a value in the function's own body.
    pub ret: Option<Ref>,
    pub wrapper: Option<Wrapper>,
    pub offset: u32,
    /// `---@param` types by parameter (empty when none is documented).
    pub param_types: Vec<Option<DocType>>,
    /// `---@return` types.
    pub returns: Vec<DocType>,
    /// By parameter (with `param_types`): the body tests it for nil (`if not
    /// x`, `x or d`, `x == nil`, `type(x)`), so a caller may leave it out
    /// whatever its doc type says.
    pub nil_ok: Vec<bool>,
}

/// `---@class Name: Parent` with the `---@field`s under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassDef {
    pub name: String,
    pub parents: Vec<String>,
    /// The parents as written (`Foo<integer>`): their generic arguments.
    pub parent_types: Vec<DocType>,
    pub fields: Vec<DocField>,
    /// `---@operator add(vector3|number): vector3`: (operator, operand, result).
    pub operators: Vec<(String, Option<DocType>, DocType)>,
    /// `---@field [integer] boolean`: (key type, value type).
    pub indexes: Vec<(DocType, DocType)>,
    /// `---@class Box<T>`: `T`.
    pub generics: Vec<String>,
    /// `---@class (exact) Name`: no field beyond the declared ones.
    pub exact: bool,
    /// `---@class (partial) Name: Parent`: the parent's fields are not required.
    pub partial: bool,
    pub offset: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocField {
    pub name: String,
    pub ty: DocType,
    pub desc: String,
    /// `---@field private x`, `protected`, `package`; None: public.
    pub visibility: Option<crate::luacats::Visibility>,
    pub offset: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportDef {
    pub name: String,
    pub value: Ref,
    pub offset: u32,
    pub doc: String,
}

/// `wrapper('Name', value)`: a call that may register an export through a
/// function found elsewhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportCall {
    pub callee: Ref,
    pub name: String,
    pub value: Ref,
    pub offset: u32,
    pub doc: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileApi {
    /// Sorted by owner (stable): a query finds an owner's defs by binary search.
    pub defs: Vec<Def>,
    pub funcs: Vec<FuncDef>,
    pub tables: u32,
    /// The value the chunk returns (a module for `require`).
    pub ret: Option<Ref>,
    pub exports: Vec<ExportDef>,
    pub export_calls: Vec<ExportCall>,
    /// Callees of `wrapper(name, value)` calls whose name is not a literal
    /// (`Export(names[i], fn)` in a loop): what they register is not known.
    pub dynamic_export_calls: Vec<Ref>,
    /// `for k, v in pairs(T) do exports(k, v) end`: every function in `T`.
    pub exports_from: Vec<Ref>,
    /// `exports(name, ...)` with a name that is not known.
    pub dynamic_exports: bool,
    /// String literals naming an export event (`__cfx_export_res_...`): exports
    /// registered by hand, for any resource.
    pub export_events: Vec<String>,
    /// Event names the file registers a handler for, with the literal's offset.
    pub events_handled: Vec<(String, u32)>,
    /// Event names the file triggers.
    pub events_triggered: Vec<(String, u32)>,
    /// Of those, the ones sent to the server (`TriggerServerEvent`,
    /// `TriggerLatentServerEvent`) and the ones fired in the same state
    /// (`TriggerEvent`), each name once.
    pub server_bound: Vec<String>,
    pub fired_locally: Vec<String>,
    /// Other string literals that look like event names (`a:b`, no spaces): an
    /// event name passed to a wrapper or kept in a table, each once.
    pub event_strings: Vec<String>,
    /// The `---@class` blocks of the file.
    pub classes: Vec<ClassDef>,
    /// Values a `---@class` comment names (`---@class xPlayer` above
    /// `local self = {}`): what is set on them belongs to the class.
    pub class_binds: Vec<(Owner, String)>,
    /// `---@alias` and `---@enum` names of the file.
    pub doc_names: Vec<String>,
    /// What each `---@alias` and `---@enum` of the file stands for.
    pub aliases: Vec<(String, DocType)>,
}

impl FileApi {
    /// The defs attached to an owner.
    pub fn defs_of(&self, owner: &Owner) -> &[Def] {
        let lo = self.defs.partition_point(|d| d.owner < *owner);
        let hi = lo + self.defs[lo..].partition_point(|d| d.owner == *owner);
        &self.defs[lo..hi]
    }

    /// Equal but for offsets: an edit that moves definitions without changing
    /// them does not touch other files.
    pub fn same_shape(&self, other: &FileApi) -> bool {
        fn all<T>(a: &[T], b: &[T], eq: impl Fn(&T, &T) -> bool) -> bool {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| eq(x, y))
        }
        all(&self.defs, &other.defs, |a, b| {
            a.owner == b.owner && a.path == b.path && a.value == b.value && deprecated(&a.doc) == deprecated(&b.doc)
        }) && all(&self.funcs, &other.funcs, |a, b| {
            a.params == b.params
                && a.method == b.method
                && a.ret == b.ret
                && a.wrapper == b.wrapper
                && a.param_types == b.param_types
                && a.returns == b.returns
                && a.nil_ok == b.nil_ok
        }) && all(&self.classes, &other.classes, |a, b| {
            a.name == b.name
                && a.parents == b.parents
                && all(&a.fields, &b.fields, |x, y| x.name == y.name && x.ty == y.ty && x.desc == y.desc)
                && a.operators == b.operators
                && a.indexes == b.indexes
                && a.generics == b.generics
                && a.exact == b.exact
                && a.partial == b.partial
        }) && self.class_binds == other.class_binds && self.doc_names == other.doc_names && self.aliases == other.aliases && all(&self.exports, &other.exports, |a, b| {
            a.name == b.name && a.value == b.value && deprecated(&a.doc) == deprecated(&b.doc)
        }) && all(&self.export_calls, &other.export_calls, |a, b| {
            a.callee == b.callee && a.name == b.name && a.value == b.value
        }) && self.dynamic_export_calls == other.dynamic_export_calls
            && all(&self.events_handled, &other.events_handled, |a, b| a.0 == b.0)
            && all(&self.events_triggered, &other.events_triggered, |a, b| a.0 == b.0)
            && self.server_bound == other.server_bound
            && self.fired_locally == other.fired_locally
            && self.event_strings == other.event_strings
            && self.ret == other.ret
            && self.exports_from == other.exports_from
            && self.dynamic_exports == other.dynamic_exports
            && self.export_events == other.export_events
    }

    pub fn deprecated_names(&self) -> impl Iterator<Item = &str> {
        let defs = self.defs.iter().filter(|d| deprecated(&d.doc).is_some()).filter_map(|d| match (&d.owner, d.path.last()) {
            (_, Some(n)) => Some(n.as_str()),
            (Owner::Global(n), None) => Some(n.as_str()),
            _ => None,
        });
        let exports = self.exports.iter().filter(|e| deprecated(&e.doc).is_some()).map(|e| e.name.as_str());
        let calls = self.export_calls.iter().filter(|e| deprecated(&e.doc).is_some()).map(|e| e.name.as_str());
        defs.chain(exports).chain(calls)
    }
}

/// The text after `@deprecated` (possibly empty), when the doc has the tag.
pub fn deprecated(doc: &str) -> Option<&str> {
    doc.lines().find_map(|l| l.trim().strip_prefix("@deprecated")).map(|d| d.trim().trim_start_matches('#').trim_start())
}

/// A declared type that says more than the code: not `table`, `any` or the
/// like (ESX 1.12's `---@return table` must not hide the value returned).
pub fn strong_type(t: &DocType) -> bool {
    match t {
        DocType::Name(n) => !matches!(n.as_str(), "table" | "any" | "unknown" | "nil" | "function" | "userdata" | "lightuserdata" | "thread"),
        DocType::Optional(inner) => strong_type(inner),
        DocType::Union(parts) => parts.iter().any(strong_type),
        DocType::Dots => false,
        _ => true,
    }
}

/// The names `---@alias Name` and `---@enum Name` lines of a text declare.
pub fn doc_type_names(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some(doc) = line.trim_start().strip_prefix("---") else { continue };
        let doc = doc.trim_start();
        let Some(rest) = doc.strip_prefix("@alias").or_else(|| doc.strip_prefix("@enum")) else { continue };
        if !rest.starts_with([' ', '\t']) {
            continue;
        }
        let rest = rest.trim_start();
        // `---@enum (key) Name`.
        let rest = rest.strip_prefix("(key)").map_or(rest, str::trim_start);
        let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '-')).collect();
        if !name.is_empty() && !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// The `---@class` blocks of a text: runs of `---` lines, each `---@field`
/// joining the class above it in the run.
pub fn doc_classes(text: &str) -> Vec<ClassDef> {
    fn flush(run: &mut Vec<(TextSize, &str)>, has_class: &mut bool, out: &mut Vec<ClassDef>) {
        if std::mem::take(has_class) {
            let (tags, _) = crate::luacats::parse_lines(run.drain(..));
            let mut cur = None;
            for t in tags {
                let offset = u32::from(t.name.map_or(t.range.start(), |r| r.start()));
                match t.tag {
                    Tag::Class { name, parents: parent_types, generics, exact, partial } => {
                        let parents = parent_types
                            .iter()
                            .filter_map(|p| match p {
                                DocType::Name(n) | DocType::Generic(n, _) => Some(n.clone()),
                                _ => None,
                            })
                            .collect();
                        out.push(ClassDef { name, parents, parent_types, fields: Vec::new(), operators: Vec::new(), indexes: Vec::new(), generics, exact, partial, offset });
                        cur = Some(out.len() - 1);
                    }
                    Tag::Operator { op, param, ret: Some(ret) } => {
                        if let Some(c) = cur {
                            out[c].operators.push((op, param, ret));
                        }
                    }
                    Tag::Field { key: Key::Name(name), ty, desc, optional, visibility } => {
                        if let Some(c) = cur {
                            let ty = if optional { DocType::Optional(Box::new(ty)) } else { ty };
                            let visibility = visibility.filter(|v| *v != crate::luacats::Visibility::Public);
                            out[c].fields.push(DocField { name, ty, desc, visibility, offset });
                        }
                    }
                    Tag::Field { key: Key::Index(k), ty, optional, .. } => {
                        if let Some(c) = cur {
                            let ty = if optional { DocType::Optional(Box::new(ty)) } else { ty };
                            out[c].indexes.push((k, ty));
                        }
                    }
                    _ => {}
                }
            }
        }
        run.clear();
    }
    let mut out = Vec::new();
    let mut run = Vec::new();
    let mut has_class = false;
    let mut at = 0usize;
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        let t = body.trim_start();
        if let Some(doc) = t.strip_prefix("---") {
            let lead = body.len() - t.len();
            run.push((TextSize::from((at + lead + 2) as u32), &t[2..]));
            has_class |= doc.trim_start().starts_with("@class");
        } else {
            flush(&mut run, &mut has_class, &mut out);
        }
        at += line.len();
    }
    flush(&mut run, &mut has_class, &mut out);
    out
}

/// `r` is built on the local `d`.
fn mentions(r: &Ref, d: u32) -> bool {
    match r {
        Ref::Local(x) => *x == d,
        Ref::Field(inner, _) | Ref::Call(inner) => mentions(inner, d),
        _ => false,
    }
}

/// How deeply a `Ref` nests.
fn ref_depth(r: &Ref) -> usize {
    match r {
        Ref::Field(inner, _) | Ref::Call(inner) => 1 + ref_depth(inner),
        _ => 0,
    }
}

/// Expressions are followed this deep (fields, calls, nested tables).
const MAX_DEPTH: usize = 12;
/// A constructor with more keyed fields than this is data: the tables under
/// its keys are not followed.
const MAX_RECORD_FIELDS: usize = 32;
/// Parents walked up to find the function a `return` belongs to.
const MAX_UP: usize = 64;

struct Builder<'a> {
    text: &'a str,
    names: &'a Names,
    root: SyntaxNode,
    api: FileApi,
    /// Function index by its `FUNC_BODY` start; table index by its start.
    funcs: HashMap<TextSize, u32>,
    tables: HashMap<TextSize, u32>,
    /// Local declaration index by its name token start.
    decls: HashMap<TextSize, u32>,
    /// Locals declared in the chunk's own block: only their values are kept
    /// (another file reaches a local only through a module or export that
    /// returns it, and those are nearly always file-level).
    top: std::collections::HashSet<u32>,
    returns: Vec<(TextSize, Ref)>,
    wrappers: Vec<(TextSize, Wrapper)>,
    /// Locals with a declared type (`---@type`, `---@param`).
    typed: HashMap<u32, DocType>,
    /// Locals a `---@class` comment names.
    bound: HashMap<u32, String>,
    /// The first value of a local inside a function: another file reaches it
    /// only through what the function returns or stores (`ESX.Players[id] =
    /// xPlayer`), so that value stands in for it.
    inner: HashMap<u32, Ref>,
}

/// The comment lines a statement starts with (its node begins at the comments
/// above it).
pub(crate) fn leading_doc(text: &str, owner: TextRange) -> String {
    text[owner]
        .lines()
        .take_while(|l| l.trim_start().starts_with("--"))
        .map(|l| l.trim_start().trim_start_matches('-').trim())
        // Checker directives, not documentation.
        .filter(|l| !l.starts_with("@diagnostic"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn string_value(text: &str, t: &SyntaxToken) -> Option<String> {
    (t.kind() == STRING).then(|| crate::util::unquote(&text[t.text_range()]).map(str::to_string)).flatten()
}

/// A string literal expression (or the bare string argument of `f 'x'`).
fn literal_string(text: &str, e: &cfx_lua_syntax::SyntaxElement) -> Option<String> {
    match e {
        rowan::NodeOrToken::Token(t) => string_value(text, t),
        rowan::NodeOrToken::Node(n) if n.kind() == LITERAL_EXPR => string_value(text, &n.first_token()?),
        _ => None,
    }
}

/// The arguments of a call: expressions, and a bare string or table argument.
fn args(call: &SyntaxNode) -> Vec<cfx_lua_syntax::SyntaxElement> {
    let Some(list) = call.children().find(|c| c.kind() == ARG_LIST) else { return Vec::new() };
    list.children_with_tokens()
        .filter(|e| match e {
            rowan::NodeOrToken::Node(_) => true,
            rowan::NodeOrToken::Token(t) => matches!(t.kind(), STRING | LONG_STRING),
        })
        .collect()
}

/// `exports.res` / `exports['res']`: the resource name.
pub(crate) fn exports_resource(text: &str, names: &Names, e: &SyntaxNode) -> Option<String> {
    if e.kind() != INDEX_EXPR {
        return None;
    }
    let object = e.first_child()?;
    if !is_exports(text, names, &object) {
        return None;
    }
    if let Some(f) = crate::lint::field_name(e) {
        return Some(f.text().to_string());
    }
    let key = e.children().nth(1)?;
    (key.kind() == LITERAL_EXPR).then(|| string_value(text, &key.first_token()?)).flatten()
}

/// `exports` naming FiveM's exports table: the global, or a local set from it
/// (`local exports = assert(exports)`), not a module's own `local exports = {}`.
pub(crate) fn is_exports(text: &str, names: &Names, name: &SyntaxNode) -> bool {
    let Some(t) = name.first_token() else { return false };
    if name.kind() != NAME_EXPR || t.text() != "exports" {
        return false;
    }
    match name_target(names, t.text_range()) {
        Some(Target::Global) => true,
        Some(Target::Decl(d)) => {
            let stat = &text[names.decls[d].owner];
            stat.trim_start().starts_with("local")
                && stat.split_once('=').is_some_and(|(_, rhs)| {
                    rhs.split(|c: char| !(c.is_alphanumeric() || c == '_')).any(|w| w == "exports")
                })
        }
        None => false,
    }
}

fn is_global(names: &Names, r: TextRange) -> bool {
    names
        .refs
        .binary_search_by_key(&r.start(), |x| x.range.start())
        .is_ok_and(|i| names.refs[i].target == Target::Global)
}

/// `_G.X` and `_ENV.X` (no local `_ENV`) are the global `X`, as in LuaLS: ox_lib
/// builds `local lib` and publishes it with `_ENV.lib = lib`.
fn env_global(mut owner: Owner, mut path: Vec<String>) -> (Owner, Vec<String>) {
    while matches!(&owner, Owner::Global(g) if g == "_G" || g == "_ENV") && !path.is_empty() {
        owner = Owner::Global(path.remove(0));
    }
    (owner, path)
}

fn name_target(names: &Names, r: TextRange) -> Option<Target> {
    names.refs.binary_search_by_key(&r.start(), |x| x.range.start()).ok().map(|i| names.refs[i].target)
}

/// The method name token of `a:b(...)`.
pub(crate) fn method_name(call: &SyntaxNode) -> Option<SyntaxToken> {
    let mut colon = false;
    for t in call.children_with_tokens().filter_map(|e| e.into_token()) {
        if t.kind() == COLON {
            colon = true;
        } else if colon && t.kind() == NAME {
            return Some(t);
        }
    }
    None
}

impl Builder<'_> {
    fn owner_of(&self, name: &SyntaxToken) -> Option<Owner> {
        match name_target(self.names, name.text_range())? {
            Target::Global => Some(Owner::Global(name.text().to_string())),
            Target::Decl(d) => Some(Owner::Local(d as u32)),
        }
    }

    /// `a`, `a.b.c`, `a['b']`: the owner and the field path.
    fn place(&self, e: &SyntaxNode) -> Option<(Owner, Vec<String>, u32)> {
        let mut path = Vec::new();
        let mut e = e.clone();
        let mut offset = None;
        while e.kind() == INDEX_EXPR {
            if path.len() == MAX_DEPTH {
                return None;
            }
            let (name, at) = match crate::lint::field_name(&e) {
                Some(f) => (f.text().to_string(), f.text_range().start()),
                None => {
                    let key = e.children().nth(1)?;
                    // A computed key (`t[id]`) is any element: `[]`.
                    let s = (key.kind() == LITERAL_EXPR)
                        .then(|| string_value(self.text, &key.first_token()?))
                        .flatten()
                        .unwrap_or_else(|| "[]".to_string());
                    (s, key.text_range().start())
                }
            };
            offset.get_or_insert(u32::from(at));
            path.push(name);
            e = e.first_child()?;
        }
        if e.kind() != NAME_EXPR {
            return None;
        }
        let t = e.first_token()?;
        path.reverse();
        let (owner, path) = env_global(self.owner_of(&t)?, path);
        Some((owner, path, offset.unwrap_or(u32::from(t.text_range().start()))))
    }

    fn func(&mut self, body: &SyntaxNode, method: bool, offset: TextSize) -> u32 {
        if let Some(&i) = self.funcs.get(&body.text_range().start()) {
            return i;
        }
        let params = body
            .children()
            .find(|c| c.kind() == PARAM_LIST)
            .map(|l| {
                l.children()
                    .filter(|p| p.kind() == PARAM)
                    .filter_map(|p| p.first_token().map(|t| t.text().to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let i = self.api.funcs.len() as u32;
        self.api.funcs.push(FuncDef {
            params,
            method,
            ret: None,
            wrapper: None,
            offset: u32::from(offset),
            param_types: Vec::new(),
            returns: Vec::new(),
            nil_ok: Vec::new(),
        });
        self.funcs.insert(body.text_range().start(), i);
        i
    }

    fn reference(&mut self, e: &SyntaxNode, depth: usize) -> Ref {
        if depth > MAX_DEPTH {
            return Ref::Unknown;
        }
        match e.kind() {
            NAME_EXPR => {
                let Some(t) = e.first_token() else { return Ref::Unknown };
                match name_target(self.names, t.text_range()) {
                    Some(Target::Global) => Ref::Global(t.text().to_string()),
                    Some(Target::Decl(d)) => self.local_ref(d as u32),
                    None => Ref::Unknown,
                }
            }
            PAREN_EXPR => e.first_child().map(|c| self.reference(&c, depth + 1)).unwrap_or(Ref::Unknown),
            INDEX_EXPR => {
                let Some(object) = e.first_child() else { return Ref::Unknown };
                let field = match crate::lint::field_name(e) {
                    Some(f) => f.text().to_string(),
                    None => match e.children().nth(1).filter(|k| k.kind() == LITERAL_EXPR).and_then(|k| string_value(self.text, &k.first_token()?)) {
                        Some(s) => s,
                        None => "[]".to_string(),
                    },
                };
                Ref::Field(Box::new(self.reference(&object, depth + 1)), field)
            }
            FUNCTION_EXPR => match e.children().find(|c| c.kind() == FUNC_BODY) {
                Some(body) => Ref::Func(self.func(&body, false, e.text_range().start())),
                None => Ref::Unknown,
            },
            TABLE_EXPR => {
                let start = e.text_range().start();
                if let Some(&i) = self.tables.get(&start) {
                    return Ref::Table(i);
                }
                let i = self.api.tables;
                self.api.tables += 1;
                self.tables.insert(start, i);
                // A data table (vehicle lists, item lists) keeps its keys, not the
                // tables under them.
                let data = e.children().filter(|f| matches!(f.kind(), FIELD_NAMED | FIELD_INDEXED)).nth(MAX_RECORD_FIELDS).is_some();
                for field in e.children() {
                    let (name, at, value) = match field.kind() {
                        FIELD_NAMED => {
                            let Some(n) = field.children_with_tokens().filter_map(|x| x.into_token()).find(|t| t.kind() == NAME) else { continue };
                            (n.text().to_string(), n.text_range().start(), field.children().last())
                        }
                        FIELD_INDEXED => {
                            let mut c = field.children();
                            let Some(key) = c.next().filter(|k| k.kind() == LITERAL_EXPR) else { continue };
                            let Some(s) = key.first_token().and_then(|t| string_value(self.text, &t)) else { continue };
                            (s, key.text_range().start(), c.next())
                        }
                        _ => continue,
                    };
                    let value = match value {
                        Some(v) if !(data && v.kind() == TABLE_EXPR) => self.reference(&v, depth + 1),
                        _ => Ref::Unknown,
                    };
                    let doc = leading_doc(self.text, field.text_range());
                    self.api.defs.push(Def { owner: Owner::Table(i), path: vec![name], value, offset: u32::from(at), doc });
                }
                Ref::Table(i)
            }
            CALL_EXPR => {
                let Some(callee) = e.first_child() else { return Ref::Unknown };
                if callee.kind() == NAME_EXPR && callee.first_token().is_some_and(|t| is_global(self.names, t.text_range())) {
                    let a = args(e);
                    match &self.text[callee.text_range()] {
                        "require" => {
                            if let Some(s) = a.first().and_then(|x| literal_string(self.text, x)) {
                                return Ref::Require(s);
                            }
                            return Ref::Unknown;
                        }
                        "setmetatable" => {
                            return match a.first() {
                                Some(rowan::NodeOrToken::Node(n)) => self.reference(n, depth + 1),
                                _ => Ref::Unknown,
                            };
                        }
                        _ => {}
                    }
                }
                Ref::Call(Box::new(self.reference(&callee, depth + 1)))
            }
            METHOD_CALL_EXPR => {
                let (Some(object), Some(m)) = (e.first_child(), method_name(e)) else { return Ref::Unknown };
                if let Some(res) = exports_resource(self.text, self.names, &object) {
                    return Ref::Call(Box::new(Ref::Export(res, m.text().to_string())));
                }
                Ref::Call(Box::new(Ref::Field(Box::new(self.reference(&object, depth + 1)), m.text().to_string())))
            }
            _ => Ref::Unknown,
        }
    }

    fn def(&mut self, owner: Owner, path: Vec<String>, value: Ref, offset: u32, stat: &SyntaxNode) {
        if let Owner::Local(d) = owner
            && ((!self.top.contains(&d) && !self.bound.contains_key(&d)) || (path.is_empty() && mentions(&value, d)))
        {
            // Not kept, or defined through itself (`x = x[k]`, `s = s:gsub(...)`):
            // nothing to follow there but a cycle.
            return;
        }
        let doc = leading_doc(self.text, stat.text_range());
        self.api.defs.push(Def { owner, path, value, offset, doc });
    }

    fn stat(&mut self, n: &SyntaxNode) {
        match n.kind() {
            LOCAL_STAT => {
                let locals: Vec<SyntaxToken> = n
                    .children()
                    .filter(|c| c.kind() == LOCAL_NAME)
                    .filter_map(|c| c.first_token())
                    .collect();
                let values: Vec<SyntaxNode> =
                    n.children().find(|c| c.kind() == EXPR_LIST).map(|l| l.children().collect()).unwrap_or_default();
                let tags = self.stat_tags(n);
                if let Some(d0) = locals.first().and_then(|t| self.decls.get(&t.text_range().start()).copied()) {
                    self.declare(d0, &tags);
                }
                for (t, v) in locals.iter().zip(values.iter()) {
                    let Some(&d) = self.decls.get(&t.text_range().start()) else { continue };
                    let mut value = self.reference(v, 0);
                    if let (Some(c), Ref::Table(i)) = (self.bound.get(&d), &value) {
                        self.api.class_binds.push((Owner::Table(*i), c.clone()));
                    }
                    if let Some(f) = self.func_of(v) {
                        self.func_tags(f, &tags);
                    }
                    if let Some(ty) = self.typed.get(&d) {
                        value = Ref::Doc(ty.clone());
                    }
                    if !self.top.contains(&d) && value != Ref::Unknown && ref_depth(&value) <= MAX_DEPTH {
                        self.inner.entry(d).or_insert_with(|| value.clone());
                    }
                    if value != Ref::Unknown {
                        self.def(Owner::Local(d), Vec::new(), value, u32::from(t.text_range().start()), n);
                    }
                }
                // `---@type A` over `local t` with no value: the type is its value.
                for t in locals.iter().skip(values.len()) {
                    let Some(&d) = self.decls.get(&t.text_range().start()) else { continue };
                    if let Some(ty) = self.typed.get(&d).cloned() {
                        self.def(Owner::Local(d), Vec::new(), Ref::Doc(ty), u32::from(t.text_range().start()), n);
                    }
                }
            }
            LOCAL_FUNCTION_STAT => {
                let Some(t) = n.children_with_tokens().filter_map(|e| e.into_token()).find(|t| t.kind() == NAME) else { return };
                let (Some(&d), Some(body)) = (self.decls.get(&t.text_range().start()), n.children().find(|c| c.kind() == FUNC_BODY))
                else {
                    return;
                };
                let f = self.func(&body, false, t.text_range().start());
                let tags = self.stat_tags(n);
                self.func_tags(f, &tags);
                if !self.top.contains(&d) {
                    self.inner.entry(d).or_insert(Ref::Func(f));
                }
                self.def(Owner::Local(d), Vec::new(), Ref::Func(f), u32::from(t.text_range().start()), n);
            }
            FUNCTION_STAT => {
                let (Some(name), Some(body)) = (n.children().find(|c| c.kind() == FUNC_NAME), n.children().find(|c| c.kind() == FUNC_BODY))
                else {
                    return;
                };
                let parts: Vec<SyntaxToken> =
                    name.children_with_tokens().filter_map(|e| e.into_token()).filter(|t| t.kind() == NAME).collect();
                let method = name.children_with_tokens().any(|e| e.kind() == COLON);
                let (Some(first), Some(last)) = (parts.first(), parts.last()) else { return };
                if parts.len() > MAX_DEPTH {
                    return;
                }
                let Some(owner) = self.owner_of(first) else { return };
                let f = self.func(&body, method, last.text_range().start());
                let tags = self.stat_tags(n);
                self.func_tags(f, &tags);
                let (owner, path) = env_global(owner, parts[1..].iter().map(|t| t.text().to_string()).collect());
                self.def(owner, path, Ref::Func(f), u32::from(last.text_range().start()), n);
            }
            ASSIGN_STAT => {
                let targets: Vec<SyntaxNode> =
                    n.children().find(|c| c.kind() == VAR_LIST).map(|l| l.children().collect()).unwrap_or_default();
                let values: Vec<SyntaxNode> =
                    n.children().find(|c| c.kind() == EXPR_LIST).map(|l| l.children().collect()).unwrap_or_default();
                let tags = if n.first_token().is_some_and(|t| t.kind() == COMMENT) { self.stat_tags(n) } else { Vec::new() };
                for (i, t) in targets.iter().enumerate() {
                    let Some((owner, path, offset)) = self.place(t) else { continue };
                    let mut value = values.get(i).map(|v| self.reference(v, 0)).unwrap_or(Ref::Unknown);
                    if i == 0 {
                        if let Some(f) = values.first().and_then(|v| self.func_of(v)) {
                            self.func_tags(f, &tags);
                        }
                        for tag in &tags {
                            match tag {
                                Tag::Class { name, .. } if path.is_empty() => {
                                    self.api.class_binds.push((owner.clone(), name.clone()));
                                    if let Ref::Table(id) = value {
                                        self.api.class_binds.push((Owner::Table(id), name.clone()));
                                    }
                                }
                                Tag::Type { tys } if tys.first().is_some_and(strong_type) => value = Ref::Doc(tys[0].clone()),
                                _ => {}
                            }
                        }
                    }
                    self.def(owner, path, value, offset, n);
                }
            }
            RETURN_STAT => {
                let Some(e) = n.children().find(|c| c.kind() == EXPR_LIST).and_then(|l| l.first_child()) else { return };
                // The function the `return` belongs to, or the chunk.
                let mut up = n.parent();
                for _ in 0..MAX_UP {
                    let Some(p) = up else { return };
                    match p.kind() {
                        FUNC_BODY => {
                            let r = self.reference(&e, 0);
                            self.returns.push((p.text_range().start(), r));
                            return;
                        }
                        SOURCE_FILE => {
                            if self.api.ret.is_none() {
                                self.api.ret = Some(self.reference(&e, 0));
                            }
                            return;
                        }
                        _ => up = p.parent(),
                    }
                }
            }
            CALL_EXPR => self.call(n),
            _ => {}
        }
    }

    /// The `---@` tags of the comments a statement starts with.
    fn stat_tags(&self, n: &SyntaxNode) -> Vec<Tag> {
        let mut lines = Vec::new();
        for e in n.children_with_tokens() {
            match e {
                rowan::NodeOrToken::Token(t) if t.kind() == COMMENT => {
                    if let Some(rest) = self.text[t.text_range()].strip_prefix("--") {
                        lines.push((t.text_range().start() + TextSize::from(2), rest));
                    }
                }
                rowan::NodeOrToken::Token(t) if t.kind() == WHITESPACE => {}
                _ => break,
            }
        }
        if lines.is_empty() {
            return Vec::new();
        }
        crate::luacats::parse_lines(lines).0.into_iter().map(|t| t.tag).collect()
    }

    /// A local's `---@class` or `---@type`.
    fn declare(&mut self, d: u32, tags: &[Tag]) {
        for tag in tags {
            match tag {
                Tag::Class { name, .. } => {
                    self.bound.insert(d, name.clone());
                    self.api.class_binds.push((Owner::Local(d), name.clone()));
                    return;
                }
                Tag::Type { tys } if tys.first().is_some_and(strong_type) => {
                    self.typed.insert(d, tys[0].clone());
                }
                _ => {}
            }
        }
    }

    /// A local as a value: its declared type, its class, or (inside a
    /// function) its first value.
    fn local_ref(&self, d: u32) -> Ref {
        if let Some(c) = self.bound.get(&d) {
            return Ref::Doc(DocType::Name(c.clone()));
        }
        if let Some(t) = self.typed.get(&d) {
            return Ref::Doc(t.clone());
        }
        if !self.top.contains(&d)
            && let Some(r) = self.inner.get(&d)
        {
            return r.clone();
        }
        Ref::Local(d)
    }

    /// The function a `function () end` expression is.
    fn func_of(&self, v: &SyntaxNode) -> Option<u32> {
        if v.kind() != FUNCTION_EXPR {
            return None;
        }
        let body = v.children().find(|c| c.kind() == FUNC_BODY)?;
        self.funcs.get(&body.text_range().start()).copied()
    }

    /// `---@param` / `---@return` on a function.
    fn func_tags(&mut self, f: u32, tags: &[Tag]) {
        if tags.is_empty() {
            return;
        }
        let Some(&body_start) = self.funcs.iter().find(|(_, i)| **i == f).map(|(s, _)| s) else { return };
        let params: Vec<(String, Option<u32>)> = self.param_decls(body_start);
        let mut types = vec![None; params.len()];
        let mut returns = Vec::new();
        for tag in tags {
            match tag {
                Tag::Param { name, ty: Some(ty), optional, .. } => {
                    let Some(i) = params.iter().position(|(p, _)| p == name) else { continue };
                    if strong_type(ty)
                        && let Some(d) = params[i].1
                    {
                        self.typed.insert(d, ty.clone());
                        // The editor's full summary keeps every local: the
                        // parameter's type is its value there.
                        if self.top.contains(&d) {
                            let offset = u32::from(self.names.decls[d as usize].range.start());
                            self.api.defs.push(Def { owner: Owner::Local(d), path: Vec::new(), value: Ref::Doc(ty.clone()), offset, doc: String::new() });
                        }
                    }
                    types[i] = Some(if *optional { DocType::Optional(Box::new(ty.clone())) } else { ty.clone() });
                }
                Tag::Return { rets, .. } => returns.extend(rets.iter().map(|r| r.ty.clone())),
                _ => {}
            }
        }
        let body = self
            .root
            .token_at_offset(body_start)
            .right_biased()
            .and_then(|t| t.parent_ancestors().find(|a| a.kind() == FUNC_BODY && a.text_range().start() == body_start))
            .map(|b| b.text_range());
        let nil_ok: Vec<bool> = match body {
            Some(body) if types.iter().any(Option::is_some) => params.iter().map(|(_, d)| d.is_some_and(|d| self.nil_tested(d, body))).collect(),
            _ => Vec::new(),
        };
        let fd = &mut self.api.funcs[f as usize];
        if types.iter().any(Option::is_some) {
            fd.param_types = types;
            fd.nil_ok = nil_ok;
        }
        if let Some(first) = returns.first()
            && strong_type(first)
            && fd.ret.is_none()
        {
            fd.ret = Some(Ref::Doc(first.clone()));
        }
        fd.returns = returns;
    }

    /// A local the code tests for nil: `not x`, `x == nil` / `x ~= nil`, `x or
    /// d`, `x and ...`, `if x then`, `type(x)`.
    fn nil_tested(&self, d: u32, body: TextRange) -> bool {
        let refs = &self.names.refs;
        let from = refs.partition_point(|r| r.range.start() < body.start());
        refs[from..].iter().take_while(|r| r.range.start() < body.end()).filter(|r| r.target == Target::Decl(d as usize)).any(|r| {
            let Some(n) = self.root.token_at_offset(r.range.start()).right_biased().and_then(|t| t.parent()) else { return false };
            if n.kind() != NAME_EXPR {
                return false;
            }
            let Some(p) = n.parent() else { return false };
            let op = || p.children_with_tokens().filter_map(|e| e.into_token()).find(|t| !t.kind().is_trivia()).map(|t| t.kind());
            match p.kind() {
                UNARY_EXPR => op() == Some(NOT_KW),
                BINARY_EXPR => match op() {
                    Some(OR_KW | AND_KW) => p.first_child().as_ref() == Some(&n),
                    Some(EQ2 | NEQ) => p.children().any(|c| c != n && c.kind() == LITERAL_EXPR && c.first_token().is_some_and(|t| t.kind() == NIL_KW)),
                    _ => false,
                },
                IF_STAT | ELSEIF_CLAUSE | WHILE_STAT => true,
                ARG_LIST => p.parent().and_then(|c| c.first_child()).is_some_and(|c| c.text() == "type"),
                _ => false,
            }
        })
    }

    /// The parameters of the function whose body starts there, with their
    /// declaration index.
    fn param_decls(&self, body_start: TextSize) -> Vec<(String, Option<u32>)> {
        let Some(tok) = self.root.token_at_offset(body_start).right_biased() else { return Vec::new() };
        let Some(body) = tok.parent_ancestors().find(|a| a.kind() == FUNC_BODY && a.text_range().start() == body_start) else {
            return Vec::new();
        };
        body.children()
            .find(|c| c.kind() == PARAM_LIST)
            .map(|l| {
                l.children()
                    .filter(|p| p.kind() == PARAM)
                    .filter_map(|p| p.first_token())
                    .map(|t| (t.text().to_string(), self.decls.get(&t.text_range().start()).copied()))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn enclosing_body(n: &SyntaxNode) -> Option<SyntaxNode> {
        let mut up = n.parent();
        for _ in 0..MAX_UP {
            let p = up?;
            if p.kind() == FUNC_BODY {
                return Some(p);
            }
            up = p.parent();
        }
        None
    }

    fn param_index(body: &SyntaxNode, name: &str) -> Option<usize> {
        body.children()
            .find(|c| c.kind() == PARAM_LIST)?
            .children()
            .filter(|p| p.kind() == PARAM)
            .position(|p| p.first_token().is_some_and(|t| t.text() == name))
    }

    fn call(&mut self, call: &SyntaxNode) {
        let Some(callee) = call.first_child() else { return };
        let a = args(call);
        let first_string = a.first().and_then(|x| literal_string(self.text, x));
        let first_offset = a.first().map(|x| u32::from(x.text_range().start())).unwrap_or(0);
        let doc = || {
            call.parent()
                .filter(|p| p.kind() == CALL_STAT)
                .map(|p| leading_doc(self.text, p.text_range()))
                .unwrap_or_default()
        };
        let global = callee.kind() == NAME_EXPR && callee.first_token().is_some_and(|t| is_global(self.names, t.text_range()));
        let callee_text = &self.text[callee.text_range()];
        // `local exports = assert(exports)`: still the global.
        let global = global || (callee_text == "exports" && is_exports(self.text, self.names, &callee));
        if global {
            match callee_text {
                "exports" => {
                    let value = match a.get(1) {
                        Some(rowan::NodeOrToken::Node(v)) => self.reference(v, 0),
                        _ => Ref::Unknown,
                    };
                    if let Some(name) = first_string {
                        let doc = doc();
                        self.api.exports.push(ExportDef { name, value, offset: first_offset, doc });
                        return;
                    }
                    // `exports(name, fn)` inside a function taking both: a wrapper.
                    let arg_name = |i: usize| match a.get(i) {
                        Some(rowan::NodeOrToken::Node(n)) if n.kind() == NAME_EXPR => Some(self.text[n.text_range()].to_string()),
                        _ => None,
                    };
                    let (k, v) = (arg_name(0), arg_name(1));
                    if let (Some(k), Some(v)) = (&k, &v)
                        && let Some(body) = Self::enclosing_body(call)
                    {
                        let key_param = Self::param_index(&body, k) == Some(0);
                        // `exports(upperName, fn)` with `fn` the second parameter.
                        let derived = !key_param && Self::param_index(&body, v) == Some(1);
                        if key_param || derived {
                            self.wrappers.push((body.text_range().start(), Wrapper { resource: None, derived }));
                            return;
                        }
                    }
                    // `for k, v in pairs(T) do exports(k, v) end`.
                    if let Some(k) = k
                        && let Some(table) = pairs_table(call, &k)
                    {
                        let r = self.reference(&table, 0);
                        self.api.exports_from.push(r);
                    }
                    self.api.dynamic_exports = true;
                }
                "RegisterNetEvent" | "AddEventHandler" | "RegisterServerEvent" => {
                    if let Some(name) = first_string.clone() {
                        self.api.events_handled.push((name, first_offset));
                    }
                }
                "TriggerEvent" | "TriggerServerEvent" | "TriggerClientEvent" | "TriggerLatentClientEvent"
                | "TriggerLatentServerEvent" => {
                    if let Some(name) = first_string.clone() {
                        let list = match callee_text {
                            "TriggerEvent" => Some(&mut self.api.fired_locally),
                            "TriggerServerEvent" | "TriggerLatentServerEvent" => Some(&mut self.api.server_bound),
                            _ => None,
                        };
                        if let Some(list) = list
                            && !list.contains(&name)
                        {
                            list.push(name.clone());
                        }
                        self.api.events_triggered.push((name, first_offset));
                    }
                }
                _ => {}
            }
        }
        // `wrapper('Name', value)`: a function whose name says export.
        let last = callee_text.rsplit(['.', ':']).next().unwrap_or("");
        // A name computed at run time; not a table (`server_exports { ... }` in a manifest).
        let computed = matches!(a.first(), Some(rowan::NodeOrToken::Node(n)) if matches!(n.kind(), NAME_EXPR | INDEX_EXPR | CALL_EXPR | METHOD_CALL_EXPR | BINARY_EXPR | PAREN_EXPR));
        if last != "exports" && last.to_ascii_lowercase().contains("export") && first_string.is_none() && computed {
            let callee_ref = self.reference(&callee, 0);
            if !self.api.dynamic_export_calls.contains(&callee_ref) {
                self.api.dynamic_export_calls.push(callee_ref);
            }
        }
        if last != "exports"
            && last.to_ascii_lowercase().contains("export")
            && let Some(name) = first_string
        {
            let value = match a.get(1) {
                Some(rowan::NodeOrToken::Node(v)) => self.reference(v, 0),
                _ => Ref::Unknown,
            };
            let callee_ref = self.reference(&callee, 0);
            let doc = doc();
            self.api.export_calls.push(ExportCall { callee: callee_ref, name, value, offset: first_offset, doc });
        }
    }

    /// `'__cfx_export_res_%s'` inside a function: a wrapper for `res`.
    fn export_event(&mut self, t: &SyntaxToken) {
        let Some(s) = string_value(self.text, t) else { return };
        let Some(rest) = s.strip_prefix("__cfx_export_") else { return };
        self.api.export_events.push(s.clone());
        if let Some(res) = rest.strip_suffix("_%s")
            && let Some(parent) = t.parent()
            && let Some(body) = Self::enclosing_body(&parent)
        {
            self.wrappers.push((body.text_range().start(), Wrapper { resource: Some(res.to_string()), derived: false }));
        }
    }
}

/// The table of `for k, v in pairs(T)` around a call, when `k` is its key variable.
fn pairs_table(call: &SyntaxNode, key: &str) -> Option<SyntaxNode> {
    let mut up = call.parent();
    for _ in 0..MAX_UP {
        let p = up?;
        if p.kind() == GENERIC_FOR_STAT {
            let names: Vec<String> = p
                .children_with_tokens()
                .filter_map(|e| e.into_token())
                .take_while(|t| t.kind() != IN_KW)
                .filter(|t| t.kind() == NAME)
                .map(|t| t.text().to_string())
                .collect();
            if names.first().map(String::as_str) != Some(key) {
                return None;
            }
            let iter = p.children().find(|c| c.kind() == EXPR_LIST)?.first_child()?;
            if iter.kind() != CALL_EXPR {
                return None;
            }
            let callee = iter.first_child()?;
            if !matches!(callee.first_token().map(|t| t.text().to_string()).as_deref(), Some("pairs")) {
                return None;
            }
            return iter.children().find(|c| c.kind() == ARG_LIST)?.first_child();
        }
        up = p.parent();
    }
    None
}

/// The API summary of a file.
pub fn file_api(parse: &Parse, text: &str, names: &Names) -> FileApi {
    finish(build(parse, text, names, false))
}

/// The summary with every local kept, not only the file-level ones: the open
/// file's view, where the cursor is usually inside a function.
pub fn file_api_full(parse: &Parse, text: &str, names: &Names) -> FileApi {
    finish(build(parse, text, names, true))
}

/// The full summary (`file_api_full`) and what the expression `e` is in it.
pub fn expr_ref(parse: &Parse, text: &str, names: &Names, e: &SyntaxNode) -> (FileApi, Ref) {
    let mut b = build(parse, text, names, true);
    let r = b.reference(e, 0);
    (finish(b), r)
}

fn build<'a>(parse: &Parse, text: &'a str, names: &'a Names, all: bool) -> Builder<'a> {
    let decls = names
        .decls
        .iter()
        .enumerate()
        .map(|(i, d)| (d.range.start(), i as u32))
        .collect();
    let root = parse.syntax();
    let decls: HashMap<TextSize, u32> = decls;
    let top = if all {
        (0..names.decls.len() as u32).collect()
    } else {
        root
        .children()
        .find(|c| c.kind() == BLOCK)
        .into_iter()
        .flat_map(|b| b.children())
        .filter(|s| matches!(s.kind(), LOCAL_STAT | LOCAL_FUNCTION_STAT))
        .flat_map(|s| {
            // `local a, b` names, or the name of `local function f`.
            let names: Vec<SyntaxToken> = match s.kind() {
                LOCAL_STAT => s.children().filter(|c| c.kind() == LOCAL_NAME).filter_map(|c| c.first_token()).collect(),
                _ => s.children_with_tokens().filter_map(|e| e.into_token()).filter(|t| t.kind() == NAME).take(1).collect(),
            };
            names.into_iter().filter_map(|t| decls.get(&t.text_range().start()).copied()).collect::<Vec<_>>()
        })
        .collect()
    };
    let mut b = Builder {
        text,
        names,
        root: root.clone(),
        typed: HashMap::new(),
        bound: HashMap::new(),
        inner: HashMap::new(),
        api: FileApi::default(),
        funcs: HashMap::new(),
        tables: HashMap::new(),
        top,
        decls,
        returns: Vec::new(),
        wrappers: Vec::new(),
    };
    let mut event_like: Vec<(u32, String)> = Vec::new();
    for e in root.descendants_with_tokens() {
        match e {
            rowan::NodeOrToken::Node(n) => b.stat(&n),
            rowan::NodeOrToken::Token(t) => {
                if t.kind() == STRING && t.text().contains("__cfx_export_") {
                    b.export_event(&t);
                }
                if t.kind() == STRING
                    && t.text().len() <= 130
                    && t.text().contains(':')
                    && let Some(s) = string_value(text, &t)
                    && !s.contains(char::is_whitespace)
                    && !s.contains("//")
                {
                    event_like.push((u32::from(t.text_range().start()), s));
                }
            }
        }
    }
    // Event-like strings that are not the name argument of an event call.
    let calls: std::collections::HashSet<u32> =
        b.api.events_handled.iter().chain(&b.api.events_triggered).map(|(_, o)| *o).collect();
    for (o, s) in event_like {
        if !calls.contains(&o) && !b.api.event_strings.contains(&s) {
            b.api.event_strings.push(s);
        }
    }
    b
}

fn finish(mut b: Builder) -> FileApi {
    // Returns and wrappers of functions met as values.
    for (body, r) in std::mem::take(&mut b.returns) {
        if let Some(&f) = b.funcs.get(&body) {
            let f = &mut b.api.funcs[f as usize];
            if f.ret.is_none() {
                f.ret = Some(r);
            }
        }
    }
    for (body, w) in std::mem::take(&mut b.wrappers) {
        if let Some(&f) = b.funcs.get(&body) {
            b.api.funcs[f as usize].wrapper = Some(w);
        }
    }
    // A field set again to a value the summary cannot follow adds nothing.
    let mut seen = std::collections::HashSet::new();
    let defs = std::mem::take(&mut b.api.defs);
    b.api.defs = defs
        .into_iter()
        .filter(|d| d.value != Ref::Unknown || seen.insert((d.owner.clone(), d.path.clone())))
        .collect();
    b.api.defs.sort_by(|a, b| a.owner.cmp(&b.owner));
    b.api.defs.shrink_to_fit();
    b.api.classes = doc_classes(b.text);
    b.api.doc_names = doc_type_names(b.text);
    b.api.aliases = crate::types::file_aliases(&b.root, b.text);
    b.api.funcs.shrink_to_fit();
    b.api
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api(text: &str) -> FileApi {
        let p = crate::parse(text);
        let n = crate::names(&p, text);
        file_api(&p, text, &n)
    }

    #[test]
    fn defs_and_values() {
        let a = api(
            "---Adds money.\n---@deprecated use Pay\nfunction Core.Functions.AddMoney(src, amount) end\nlocal M = {}\nM.x = require 'mod.a'\nlocal T = setmetatable({ a = 1, b = function(y) return y end }, mt)\nreturn M\n",
        );
        let d = &a.defs[0];
        assert_eq!((d.owner.clone(), d.path.clone()), (Owner::Global("Core".into()), vec!["Functions".into(), "AddMoney".into()]));
        assert_eq!(deprecated(&d.doc), Some("use Pay"));
        assert_eq!(a.funcs[0].params, ["src", "amount"]);
        assert!(a.defs.iter().any(|d| d.path == ["x"] && d.value == Ref::Require("mod.a".into())));
        assert!(a.defs.iter().any(|d| d.owner == Owner::Table(1) && d.path == ["b"]));
        assert_eq!(a.ret, Some(Ref::Local(2)), "params src, amount come first");
        assert_eq!(a.deprecated_names().collect::<Vec<_>>(), ["AddMoney"]);
    }

    #[test]
    fn env_and_g_fields_are_globals() {
        let a = api("local t = {}
_ENV.T = t
_G.U = 1
_ENV._G.V = 2
function _G.W.f() end
");
        let globals: Vec<(String, Vec<String>)> = a
            .defs
            .iter()
            .filter_map(|d| match &d.owner {
                Owner::Global(g) => Some((g.clone(), d.path.clone())),
                _ => None,
            })
            .collect();
        for want in [("T", vec![]), ("U", vec![]), ("V", vec![]), ("W", vec!["f".to_string()])] {
            assert!(globals.contains(&(want.0.to_string(), want.1.clone())), "{want:?} in {globals:?}");
        }
        assert!(!globals.iter().any(|(g, _)| g == "_ENV" || g == "_G"), "{globals:?}");
        // A local `_ENV` is a table like any other.
        let a = api("local _ENV = {}
_ENV.X = 1
");
        assert!(!a.defs.iter().any(|d| d.owner == Owner::Global("X".into())));
    }

    #[test]
    fn exports_and_wrappers() {
        let a = api(
            "local Core = {}\nexports('GetCore', function() return Core end)\nlocal function reg(name, fn) exports(name, fn) end\nreg('Other', print)\nfor k, v in pairs(Core.Functions) do exports(k, v) end\nlocal X = exports['qb-core']:GetCoreObject()\nlocal function w(n, cb) AddEventHandler(('__cfx_export_other-res_%s'):format(n), function(set) set(cb) end) end\nTriggerServerEvent('a:b', 1)\nRegisterNetEvent('c:d', function() end)\n",
        );
        assert_eq!(a.exports[0].name, "GetCore");
        let Ref::Func(f) = a.exports[0].value else { panic!() };
        assert_eq!(a.funcs[f as usize].ret, Some(Ref::Local(0)));
        assert!(a.funcs.iter().any(|f| f.wrapper == Some(Wrapper { resource: None, derived: false })));
        assert!(a.funcs.iter().any(|f| f.wrapper == Some(Wrapper { resource: Some("other-res".into()), derived: false })));
        assert_eq!(a.export_calls.len(), 0, "`reg` does not say export");
        assert_eq!(a.exports_from, [Ref::Field(Box::new(Ref::Local(0)), "Functions".into())]);
        assert!(a.dynamic_exports);
        assert!(a.defs.iter().any(|d| d.value == Ref::Call(Box::new(Ref::Export("qb-core".into(), "GetCoreObject".into())))));
        assert_eq!(a.events_triggered[0].0, "a:b");
        assert_eq!(a.events_handled[0].0, "c:d");
        let a = api("TriggerEvent('a:b')\nTriggerServerEvent('c:d', 1)\nTriggerServerEvent('c:d', 2)\nlocal names = { buy = 'res:buy', url = 'https://x.y', sql = 'a: b' }\nRegisterNetEvent('e:f')\n");
        assert_eq!((a.fired_locally.as_slice(), a.server_bound.as_slice(), a.event_strings.as_slice()), (&["a:b".to_string()][..], &["c:d".to_string()][..], &["res:buy".to_string()][..]));
        let a = api("function Export(name, fn)\n  local upper = name:sub(1, 1):upper() .. name:sub(2)\n  exports(upper, fn)\nend\n");
        assert_eq!(a.funcs[0].wrapper, Some(Wrapper { resource: None, derived: true }));
        let a = api("for i = 1, #names do Export(names[i], fn) end\n");
        assert_eq!(a.dynamic_export_calls, [Ref::Global("Export".into())]);
        let a = api("local createExport = require 'export-function'\ncreateExport('GetCoreObject', function() return 1 end)\n");
        assert_eq!(a.export_calls[0].name, "GetCoreObject");
        assert_eq!(a.export_calls[0].callee, Ref::Local(0));
    }

    #[test]
    fn deep_input_stays_bounded() {
        let mut t = String::from("x = ");
        t.push_str(&"{ a = ".repeat(5_000));
        t.push('1');
        t.push_str(&" }".repeat(5_000));
        t.push_str("\ny = a");
        t.push_str(&".b".repeat(20_000));
        t.push('\n');
        let a = api(&t);
        assert!(a.defs.len() < 100);
    }
}
