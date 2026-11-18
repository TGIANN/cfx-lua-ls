//! Following the files' `api::Ref`s across files and resources: the fields of a
//! table (a framework object, ox_lib's `lib`, `Config`), a function's
//! parameters and the value it returns, a module's return, and each resource's
//! exports. Every query has a step budget and a visited set, so cycles and huge
//! workspaces stay bounded; what the budget cuts is simply not offered.

use super::{FileEntry, FileSide, Workspace, key};

mod refs;
pub use refs::{FileRanges, TypeTarget};
use crate::api::{self, Def, FuncDef, Owner, Ref};
use crate::luacats::{DocType, FunType};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Steps one query may take.
const BUDGET: usize = 6_000;
/// Members listed at most.
const MAX_MEMBERS: usize = 400;
/// Nested `resolve` calls at most: a value defined through itself
/// (`err = err:gsub(...)`) would otherwise recurse until the budget runs out,
/// deeper than the stack.
const MAX_NESTING: usize = 48;

/// The Lua state a value lives in, and the file whose locals a `Ref` names.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Cx {
    file: String,
    res: Option<usize>,
    side: FileSide,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Base {
    Global { res: Option<usize>, side: FileSide, name: String },
    Local { file: String, decl: u32 },
    Table { file: String, id: u32 },
    /// A value of a `---@class` (seen from a resource: its own declaration
    /// first, then any in the workspace, then the library).
    Class { name: String, res: Option<usize>, side: FileSide },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Anchor {
    base: Base,
    path: Vec<String>,
}

#[derive(Debug, Clone)]
enum Val {
    Anchor(Anchor),
    Func { file: String, id: u32, cx: Cx },
    /// A function known only by its doc type (`---@field getMoney fun(): number`).
    Fun(Rc<FunType>, Cx),
}

/// Type names that are not classes worth following.
fn builtin_type(n: &str) -> bool {
    matches!(
        n,
        "nil" | "any" | "unknown" | "boolean" | "true" | "false" | "number" | "integer" | "string" | "table" | "function" | "thread" | "userdata" | "lightuserdata" | "self"
    )
}

/// A field, global or export as the editor shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiItem {
    pub name: String,
    /// The parameters when the value is a function (`...` included, no `self`).
    pub params: Option<Vec<String>>,
    /// Declared with `:` (called with `:`).
    pub method: bool,
    pub doc: String,
    /// The defining file and the name's byte offset.
    pub location: Option<(PathBuf, u32)>,
    /// For exports: the side that registers it (`Shared` for both, `Unknown`
    /// when not known).
    pub side: FileSide,
    /// The declared type (`string`, `fun(minimal?: boolean): table`).
    pub ty: Option<String>,
}

impl ApiItem {
    pub fn deprecated(&self) -> Option<&str> {
        api::deprecated(&self.doc)
    }

    /// `name(a, b)` or `name`.
    pub fn signature(&self) -> String {
        match &self.params {
            Some(p) => format!("{}({})", self.name, p.join(", ")),
            None => self.name.clone(),
        }
    }
}

/// What `workspace/symbol` finds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WsSymbolKind {
    Function,
    Variable,
    Field,
    Export,
    Event,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WsSymbol {
    pub name: String,
    /// `QBCore.Functions`, `exports.qb-core`, `event`; empty for a global.
    pub container: String,
    pub kind: WsSymbolKind,
    pub path: PathBuf,
    /// The name's byte offset.
    pub offset: u32,
}

/// `query`'s characters appear in `name` in order, case aside.
fn fuzzy(name: &str, query: &str) -> bool {
    let mut q = query.chars().flat_map(char::to_lowercase).peekable();
    for c in name.chars().flat_map(char::to_lowercase) {
        if q.peek() == Some(&c) {
            q.next();
        }
    }
    q.peek().is_none()
}

/// A resource's exports seen from one side.
#[derive(Debug, Clone, Default)]
pub struct Exports {
    /// The resource that answers for the name (itself, or the one that
    /// `provide`s it).
    pub resource: String,
    /// Callable from the asking side.
    pub items: Vec<ApiItem>,
    /// Registered only on the other side.
    pub other_side: Vec<ApiItem>,
    /// Every export is known: no escrow file, no export with a computed name, no
    /// export registered by hand.
    pub complete: bool,
}

struct Q<'a> {
    ws: &'a Workspace,
    /// The open file's current summary, over the stored one.
    overlay: Option<(&'a str, &'a api::FileApi)>,
    budget: Cell<usize>,
    nesting: Cell<usize>,
    states: RefCell<HashMap<(Option<usize>, FileSide), StateFiles<'a>>>,
    exports: RefCell<HashMap<String, Option<Rc<RawExports>>>>,
}

/// A resource's exports before they are shown: resource, members, the side
/// each is registered on, complete.
type RawExports = (usize, Vec<Member>, Vec<FileSide>, bool);

/// The files whose globals a Lua state sees.
type StateFiles<'a> = Rc<Vec<&'a str>>;

/// One member while it is being collected.
/// One file's arity checks: locals assigned after their declaration, and what
/// each callee takes once found.
struct ArityCx {
    written: HashSet<usize>,
    /// (root, fields, `:` call) -> (most arguments, fewest required).
    known: HashMap<Callee, Option<(usize, usize)>>,
}

type Callee = (Ref, Vec<String>, bool);

enum Arity {
    /// No definition in the workspace.
    NotFound,
    /// A definition that is not a plain function, takes `...`, or is out of budget.
    Unknown,
    /// (most arguments, fewest required).
    Known(usize, usize),
}

/// A documented parameter a caller must pass: not optional, not nil-able, not
/// `any`.
fn required_type(t: &DocType) -> bool {
    match t {
        DocType::Optional(_) => false,
        DocType::Name(n) => !matches!(n.as_str(), "nil" | "any" | "unknown" | "..."),
        DocType::Union(ps) => ps.iter().all(required_type),
        DocType::Dots => false,
        _ => true,
    }
}

/// The most arguments a library function takes (its `---@overload`s included;
/// `None` for `...` or several declarations), and where it comes from. `None`:
/// not a library function.
fn library_arity(names: &crate::Names, callee: &cfx_lua_syntax::SyntaxNode) -> Option<(Option<usize>, crate::library::Origin)> {
    let path = crate::assist::expr_path(names, callee)?;
    let lib = crate::library::library();
    let items = lib.get(&path);
    let item = items.iter().find(|i| i.function)?;
    Some((library_max(lib, item, items.len()), item.origin))
}

fn library_max(lib: &crate::library::Library, item: &crate::library::LibItem, declarations: usize) -> Option<usize> {
    if declarations != 1 {
        return None;
    }
    let mut max = 0;
    let decl = lib.declaration(item);
    let doc = lib.doc(item);
    let lists = std::iter::once(decl.split_once('(')?.1)
        .chain(doc.lines().filter_map(|l| l.trim().strip_prefix("@overload")?.trim().strip_prefix("fun(")));
    for list in lists {
        // The parameter list up to its closing parenthesis.
        let mut depth = 0;
        let mut count = 0;
        let mut any = false;
        for c in list.chars() {
            match c {
                '(' | '<' | '{' | '[' => depth += 1,
                ')' | '>' | '}' | ']' if depth > 0 => depth -= 1,
                ')' => break,
                ',' if depth == 0 => count += 1,
                c if !c.is_whitespace() => any = true,
                _ => {}
            }
        }
        if list.split(')').next().is_some_and(|l| l.contains("...")) {
            return None;
        }
        max = max.max(if any { count + 1 } else { 0 });
    }
    Some(max)
}

struct Member {
    name: String,
    values: Vec<(Ref, Cx)>,
    doc: String,
    location: Option<(String, u32)>,
}

fn side_sees(asking: FileSide, other: FileSide) -> bool {
    match asking {
        FileSide::Client => matches!(other, FileSide::Client | FileSide::Shared | FileSide::Unknown),
        FileSide::Server => matches!(other, FileSide::Server | FileSide::Shared | FileSide::Unknown),
        FileSide::Shared | FileSide::Unknown => true,
    }
}

impl<'a> Q<'a> {
    fn new(ws: &'a Workspace, overlay: Option<(&'a str, &'a api::FileApi)>) -> Q<'a> {
        Q { ws, overlay, budget: Cell::new(BUDGET), nesting: Cell::new(0), states: RefCell::default(), exports: RefCell::default() }
    }

    fn step(&self) -> bool {
        let b = self.budget.get();
        if b == 0 {
            return false;
        }
        self.budget.set(b - 1);
        true
    }

    fn api(&self, file: &str) -> Option<&'a api::FileApi> {
        if let Some((k, a)) = self.overlay
            && k == file
        {
            return Some(a);
        }
        self.ws.files.get(file).map(|e| &e.api)
    }

    fn entry(&self, file: &str) -> Option<&'a FileEntry> {
        self.ws.files.get(file)
    }

    /// The state a file's own code runs in.
    fn cx_of(&self, file: &str, fallback: FileSide) -> Cx {
        match self.entry(file).and_then(|e| Some((e.resource?, e.rel.as_str()))) {
            Some((id, rel)) => {
                let side = match self.ws.resources[id].manifest.side(rel) {
                    FileSide::Unknown => fallback,
                    s => s,
                };
                Cx { file: file.to_string(), res: Some(id), side }
            }
            None => Cx { file: file.to_string(), res: None, side: fallback },
        }
    }

    /// The files whose globals the state sees: the resource's files of that
    /// side and the manifest's `@res/file` imports.
    fn state_files(&self, res: Option<usize>, side: FileSide) -> Rc<Vec<&'a str>> {
        if let Some(v) = self.states.borrow().get(&(res, side)) {
            return v.clone();
        }
        let v = Rc::new(self.state_files_uncached(res, side));
        self.states.borrow_mut().insert((res, side), v.clone());
        v
    }

    fn state_files_uncached(&self, res: Option<usize>, side: FileSide) -> Vec<&'a str> {
        let ws = self.ws;
        let Some(id) = res else {
            return ws.files.iter().filter(|(_, e)| e.resource.is_none()).map(|(k, _)| k.as_str()).collect();
        };
        let r = &ws.resources[id];
        let mut out: Vec<&str> = r
            .files
            .iter()
            .filter(|f| {
                ws.files.get(*f).is_some_and(|e| r.manifest.dynamic || side_sees(side, r.manifest.side(&e.rel)))
            })
            .map(String::as_str)
            .collect();
        let m = &r.manifest;
        let lists: Vec<&Vec<String>> = match side {
            FileSide::Client => vec![&m.shared, &m.client],
            FileSide::Server => vec![&m.shared, &m.server],
            _ => vec![&m.shared, &m.client, &m.server],
        };
        for entry in lists.into_iter().flatten().filter(|p| p.starts_with('@')) {
            let Some((res, file)) = entry[1..].split_once('/') else { continue };
            let Some(&other) = ws.by_name.get(&res.to_lowercase()) else { continue };
            for f in &ws.resources[other].files {
                if ws.files.get(f).is_some_and(|e| crate::manifest::glob(file, &e.rel)) {
                    out.push(f.as_str());
                }
            }
        }
        out
    }

    /// ox_lib loads `lib.<name>` from `imports/<name>/<side>.lua` on first use.
    fn ox_modules(&self, files: &[&str]) -> Option<usize> {
        files.iter().find_map(|f| {
            let e = self.entry(f)?;
            let id = e.resource?;
            (e.rel.eq_ignore_ascii_case("init.lua")
                && self.ws.resources[id].files.iter().any(|g| self.entry(g).is_some_and(|x| x.rel.starts_with("imports/"))))
            .then_some(id)
        })
    }

    fn module_files(&self, ox: usize, name: &str, side: FileSide) -> Vec<&'a str> {
        let prefix = format!("imports/{name}/");
        self.ws.resources[ox]
            .files
            .iter()
            .filter(|f| {
                self.entry(f).is_some_and(|e| {
                    e.rel.strip_prefix(&prefix).is_some_and(|file| match file {
                        "client.lua" => side_sees(side, FileSide::Client),
                        "server.lua" => side_sees(side, FileSide::Server),
                        _ => !file.contains('/'),
                    })
                })
            })
            .map(String::as_str)
            .collect()
    }

    /// The files that may define the anchor or its fields, with their state.
    fn anchor_files(&self, a: &Anchor) -> Vec<(&'a str, Cx)> {
        match &a.base {
            Base::Class { name, res, .. } => self.class_files(name, *res).into_iter().map(|f| (f, self.cx_of(f, FileSide::Unknown))).collect(),
            Base::Local { file, .. } | Base::Table { file, .. } => {
                let Some(k) = self.ws.files.get_key_value(file.as_str()).map(|(k, _)| k.as_str()).or(self.overlay.map(|o| o.0))
                else {
                    return Vec::new();
                };
                vec![(k, self.cx_of(k, FileSide::Unknown))]
            }
            Base::Global { res, side, name } => {
                let files = self.state_files(*res, *side);
                let mut out: Vec<(&str, Cx)> =
                    files.iter().map(|f| (*f, Cx { file: f.to_string(), res: *res, side: *side })).collect();
                // The file being edited, before the index has it (a new, unsaved file).
                if let Some((k, _)) = self.overlay
                    && !self.ws.files.contains_key(k)
                    && self.cx_of(k, *side).res == *res
                {
                    out.push((k, Cx { file: k.to_string(), res: *res, side: *side }));
                }
                if name == "lib"
                    && let Some(ox) = self.ox_modules(&files)
                {
                    // What ox_lib sets on its own `lib` (`resource/**`) is exported,
                    // and an importer's `lib.x` falls back to that export.
                    if Some(ox) != *res {
                        let modules = |f: &&str| self.entry(f).is_some_and(|e| e.rel.starts_with("imports/"));
                        for f in self.state_files(Some(ox), *side).iter().copied().filter(|f| !modules(f)) {
                            out.push((f, Cx { file: f.to_string(), res: Some(ox), side: *side }));
                        }
                    }
                    if let Some(module) = a.path.first() {
                        for f in self.module_files(ox, module, *side) {
                            out.push((f, Cx { file: f.to_string(), res: *res, side: *side }));
                        }
                    }
                }
                out
            }
        }
    }

    /// The defs of the anchor itself or of its direct fields.
    fn defs(&self, a: &Anchor, fields: bool) -> Vec<(&'a Def, Cx)> {
        self.defs_at(a, &a.path, fields)
    }

    /// The defs of `a.base` + `path` (a prefix of the anchor's path).
    fn defs_at(&self, a: &Anchor, path: &[String], fields: bool) -> Vec<(&'a Def, Cx)> {
        let mut out = Vec::new();
        if let Base::Class { name, .. } = &a.base {
            for (file, cx) in self.anchor_files(a) {
                let Some(api) = self.api(file) else { continue };
                for (owner, _) in api.class_binds.iter().filter(|(_, c)| c == name) {
                    for d in api.defs_of(owner) {
                        let ok = if fields { d.path.len() == path.len() + 1 && d.path.starts_with(path) } else { d.path == path };
                        if ok {
                            out.push((d, cx.clone()));
                        }
                    }
                }
            }
            return out;
        }
        let owner = match &a.base {
            Base::Class { .. } => unreachable!(),
            Base::Global { name, .. } => Owner::Global(name.clone()),
            Base::Local { decl, .. } => Owner::Local(*decl),
            Base::Table { id, .. } => Owner::Table(*id),
        };
        for (file, cx) in self.anchor_files(a) {
            let Some(api) = self.api(file) else { continue };
            for d in api.defs_of(&owner) {
                let ok = if fields {
                        d.path.len() == path.len() + 1 && d.path.starts_with(path)
                    } else {
                        d.path == path
                    };
                if ok {
                    out.push((d, cx.clone()));
                }
            }
        }
        out
    }

    fn resolve(&self, r: &Ref, cx: &Cx, out: &mut Vec<Val>) {
        let depth = self.nesting.get();
        if depth >= MAX_NESTING || !self.step() {
            return;
        }
        self.nesting.set(depth + 1);
        self.resolve_inner(r, cx, out);
        self.nesting.set(depth);
    }

    fn resolve_inner(&self, r: &Ref, cx: &Cx, out: &mut Vec<Val>) {
        match r {
            Ref::Unknown => {}
            Ref::Global(n) => {
                out.push(Val::Anchor(Anchor { base: Base::Global { res: cx.res, side: cx.side, name: n.clone() }, path: Vec::new() }))
            }
            Ref::Local(d) => out.push(Val::Anchor(Anchor { base: Base::Local { file: cx.file.clone(), decl: *d }, path: Vec::new() })),
            Ref::Table(t) => out.push(Val::Anchor(Anchor { base: Base::Table { file: cx.file.clone(), id: *t }, path: Vec::new() })),
            Ref::Func(f) => out.push(Val::Func { file: cx.file.clone(), id: *f, cx: cx.clone() }),
            Ref::Field(object, name) => {
                let mut inner = Vec::new();
                self.resolve(object, cx, &mut inner);
                for v in inner {
                    if let Val::Anchor(mut a) = v
                        && a.path.len() < 16
                    {
                        a.path.push(name.clone());
                        out.push(Val::Anchor(a));
                    }
                }
            }
            Ref::Call(callee) => {
                let mut funcs = Vec::new();
                self.funcs_of_ref(callee, cx, &mut funcs, &mut HashSet::new());
                let found = !funcs.is_empty();
                for (f, fcx) in funcs {
                    if let Some(ret) = &f.ret {
                        self.resolve(ret, &fcx, out);
                    }
                }
                let docs = self.doc_funs(callee, cx);
                for (f, fcx) in &docs {
                    if let Some(r) = f.rets.first() {
                        self.resolve_doc(&r.ty, fcx, out);
                    }
                }
                // A library function (`GetEntityCoords`): its `---@return`.
                if !found
                    && docs.is_empty()
                    && let Ref::Global(name) = &**callee
                    && let Some(item) = crate::library::library().get(name).iter().find(|i| i.function)
                    && let Some(ty) = crate::library::returns(item).first()
                {
                    self.resolve_doc(ty, cx, out);
                }
            }
            Ref::Doc(t) => self.resolve_doc(t, cx, out),
            Ref::Require(module) => {
                if let Some(file) = self.module(module, cx)
                    && let Some(ret) = self.api(file).and_then(|a| a.ret.as_ref())
                {
                    let mcx = Cx { file: file.to_string(), res: cx.res, side: cx.side };
                    self.resolve(ret, &mcx, out);
                }
            }
            Ref::Export(res, name) => {
                for (value, vcx) in self.export_values(res, name, cx.side) {
                    self.resolve(&value, &vcx, out);
                }
            }
        }
    }

    /// The values a doc type stands for: class instances and function types.
    fn resolve_doc(&self, t: &DocType, cx: &Cx, out: &mut Vec<Val>) {
        if !self.step() {
            return;
        }
        match t {
            DocType::Optional(inner) => self.resolve_doc(inner, cx, out),
            DocType::Union(parts) => {
                for p in parts {
                    self.resolve_doc(p, cx, out);
                }
            }
            DocType::Name(n) | DocType::Generic(n, _) => {
                if !builtin_type(n) && self.class_exists(n, cx.res) {
                    let base = Base::Class { name: n.clone(), res: cx.res, side: cx.side };
                    out.push(Val::Anchor(Anchor { base, path: Vec::new() }));
                }
            }
            DocType::Fun(f) => out.push(Val::Fun(Rc::new((**f).clone()), cx.clone())),
            _ => {}
        }
    }

    fn class_exists(&self, name: &str, res: Option<usize>) -> bool {
        !self.class_files(name, res).is_empty() || crate::library::classes().contains_key(name)
    }

    /// The files that declare a class or bind values to it: the resource's own
    /// when it has any, else those of the resources its manifest names
    /// (`dependency`, `@res/` imports), else every one in the workspace; the
    /// open file too.
    fn class_files(&self, name: &str, res: Option<usize>) -> Vec<&'a str> {
        let mut out: Vec<&'a str> = Vec::new();
        if let Some(files) = self.ws.classes.get(name) {
            let of = |set: &[usize]| -> Vec<&'a str> {
                files.iter().filter(|f| self.entry(f).and_then(|e| e.resource).is_some_and(|r| set.contains(&r))).map(String::as_str).collect()
            };
            // One file: every tier gives it.
            let res = res.filter(|_| files.len() > 1);
            let own = res.map(|r| of(&[r])).unwrap_or_default();
            let related = if own.is_empty() { res.map(|r| of(&self.ws.related(r))).unwrap_or_default() } else { Vec::new() };
            out = if !own.is_empty() {
                own
            } else if !related.is_empty() {
                related
            } else {
                files.iter().map(String::as_str).collect()
            };
        }
        if let Some((k, api)) = self.overlay
            && !out.contains(&k)
            && (api.classes.iter().any(|c| c.name == name) || api.class_binds.iter().any(|(_, c)| c == name))
        {
            out.push(k);
        }
        out
    }

    /// A class's `---@field`s and parents: workspace declarations, else the
    /// library's.
    fn class_defs(&self, name: &str, res: Option<usize>) -> Vec<(&'a api::ClassDef, Option<&'a str>)> {
        let mut out = Vec::new();
        for f in self.class_files(name, res) {
            if let Some(api) = self.api(f) {
                out.extend(api.classes.iter().filter(|c| c.name == name).map(|c| (c, Some(f))));
            }
        }
        if out.is_empty()
            && let Some(lib) = crate::library::classes().get(name)
        {
            out.extend(lib.iter().map(|c| (c, None)));
        }
        out
    }

    /// The declared types of a class field, parents included.
    fn class_field_types(&self, name: &str, res: Option<usize>, field: &str, cx: &Cx) -> Vec<(DocType, Cx)> {
        let mut out = Vec::new();
        let mut todo = vec![name.to_string()];
        let mut seen = HashSet::new();
        while let Some(c) = todo.pop() {
            if !seen.insert(c.clone()) || seen.len() > 32 {
                continue;
            }
            for (def, file) in self.class_defs(&c, res) {
                let fcx = file.map_or_else(|| cx.clone(), |f| self.cx_of(f, cx.side));
                out.extend(def.fields.iter().filter(|f| f.name == field).map(|f| (f.ty.clone(), fcx.clone())));
                todo.extend(def.parents.iter().cloned());
            }
        }
        out
    }

    /// Function types a callee has from doc comments.
    fn doc_funs(&self, r: &Ref, cx: &Cx) -> Vec<(Rc<FunType>, Cx)> {
        let mut vals = Vec::new();
        self.resolve(r, cx, &mut vals);
        let mut out = Vec::new();
        for v in vals {
            match v {
                Val::Fun(f, fcx) => out.push((f, fcx)),
                Val::Anchor(a) => {
                    if let (Base::Class { name, res, .. }, [field]) = (&a.base, a.path.as_slice()) {
                        for (ty, fcx) in self.class_field_types(name, *res, field, cx) {
                            let mut inner = Vec::new();
                            self.resolve_doc(&ty, &fcx, &mut inner);
                            out.extend(inner.into_iter().filter_map(|v| match v {
                                Val::Fun(f, c) => Some((f, c)),
                                _ => None,
                            }));
                        }
                    }
                }
                Val::Func { .. } => {}
            }
        }
        out
    }

    fn type_views(&self, r: &Ref, cx: &Cx, depth: usize, out: &mut Vec<String>) {
        let mut push = |s: String| {
            if !out.contains(&s) && out.len() < 4 {
                out.push(s);
            }
        };
        if let Ref::Doc(t) = r {
            push(crate::luacats::view(t));
            return;
        }
        let mut vals = Vec::new();
        self.resolve(r, cx, &mut vals);
        for v in vals {
            match v {
                Val::Fun(f, _) => push(crate::luacats::view(&DocType::Fun(Box::new((*f).clone())))),
                Val::Anchor(a) => match (&a.base, a.path.as_slice()) {
                    (Base::Class { name, .. }, []) => push(name.clone()),
                    (Base::Class { name, res, .. }, [field]) => {
                        for (t, _) in self.class_field_types(name, *res, field, cx) {
                            push(crate::luacats::view(&t));
                        }
                    }
                    _ if depth < 3 => {
                        for (d, dcx) in self.defs(&a, false) {
                            let mut inner = Vec::new();
                            self.type_views(&d.value, &dcx, depth + 1, &mut inner);
                            for s in inner {
                                push(s);
                            }
                        }
                    }
                    _ => {}
                },
                Val::Func { .. } => {}
            }
        }
    }

    /// ox_lib's `require`: `a.b` in the resource, `@res.a.b` in another.
    fn module(&self, module: &str, cx: &Cx) -> Option<&'a str> {
        let (res, path) = match module.strip_prefix('@') {
            Some(rest) => {
                let (r, p) = rest.split_once('.')?;
                (*self.ws.by_name.get(&r.to_lowercase())?, p)
            }
            None => (cx.res?, module),
        };
        let rel = path.replace('.', "/").to_lowercase();
        let candidates = [format!("{rel}.lua"), format!("{rel}/init.lua")];
        self.ws.resources[res]
            .files
            .iter()
            .find(|f| self.entry(f).is_some_and(|e| candidates.iter().any(|c| e.rel.eq_ignore_ascii_case(c))))
            .map(String::as_str)
    }

    fn funcs_of_ref(&self, r: &Ref, cx: &Cx, out: &mut Vec<(&'a FuncDef, Cx)>, seen: &mut HashSet<Anchor>) {
        let mut vals = Vec::new();
        self.resolve(r, cx, &mut vals);
        for v in vals {
            self.funcs_of_val(&v, out, seen);
        }
    }

    fn funcs_of_val(&self, v: &Val, out: &mut Vec<(&'a FuncDef, Cx)>, seen: &mut HashSet<Anchor>) {
        match v {
            Val::Func { file, id, cx } => {
                if let Some(f) = self.api(file).and_then(|a| a.funcs.get(*id as usize)) {
                    out.push((f, cx.clone()));
                }
            }
            Val::Anchor(a) => {
                if !seen.insert(a.clone()) || !self.step() {
                    return;
                }
                for (d, cx) in self.defs(a, false) {
                    self.funcs_of_ref(&d.value, &cx, out, seen);
                }
                for alias in self.aliases(a, false) {
                    self.funcs_of_val(&Val::Anchor(alias), out, seen);
                }
            }
            Val::Fun(..) => {}
        }
    }

    fn members(&self, a: &Anchor, out: &mut Vec<Member>, seen: &mut HashSet<Anchor>) {
        if !seen.insert(a.clone()) || !self.step() || out.len() >= MAX_MEMBERS {
            return;
        }
        for (d, cx) in self.defs(a, true) {
            let Some(name) = d.path.last().filter(|n| *n != "[]") else { continue };
            let value = (d.value.clone(), cx.clone());
            match out.iter_mut().find(|m| m.name == *name) {
                Some(m) => {
                    m.values.push(value);
                    if m.doc.is_empty() {
                        m.doc = d.doc.clone();
                    }
                }
                None => out.push(Member {
                    name: name.clone(),
                    values: vec![value],
                    doc: d.doc.clone(),
                    location: Some((cx.file.clone(), d.offset)),
                }),
            }
        }
        // A class: its `---@field`s, then its parents'.
        if let Base::Class { name, res, side } = &a.base
            && a.path.is_empty()
        {
            for (def, file) in self.class_defs(name, *res) {
                let cx = match file {
                    Some(f) => self.cx_of(f, *side),
                    None => Cx { file: String::new(), res: *res, side: *side },
                };
                for f in &def.fields {
                    let value = (Ref::Doc(f.ty.clone()), cx.clone());
                    match out.iter_mut().find(|m| m.name == f.name) {
                        Some(m) => {
                            m.values.push(value);
                            if m.doc.is_empty() {
                                m.doc = f.desc.clone();
                            }
                        }
                        None => out.push(Member {
                            name: f.name.clone(),
                            values: vec![value],
                            doc: f.desc.clone(),
                            location: file.map(|k| (k.to_string(), f.offset)),
                        }),
                    }
                }
                for p in &def.parents {
                    let parent = Anchor { base: Base::Class { name: p.clone(), res: *res, side: *side }, path: Vec::new() };
                    self.members(&parent, out, seen);
                }
            }
        }
        // ox_lib's lazily loaded modules.
        if let Base::Global { name, side, .. } = &a.base
            && name == "lib"
            && a.path.is_empty()
        {
            let files: Vec<&str> = self.anchor_files(a).into_iter().map(|(f, _)| f).collect();
            if let Some(ox) = self.ox_modules(&files) {
                let mut names: Vec<(&str, &str)> = self.ws.resources[ox]
                    .files
                    .iter()
                    .filter_map(|f| {
                        let e = self.entry(f)?;
                        let rest = e.rel.strip_prefix("imports/")?;
                        let (module, file) = rest.split_once('/')?;
                        let fits = match file {
                            "client.lua" => side_sees(*side, FileSide::Client),
                            "server.lua" => side_sees(*side, FileSide::Server),
                            _ => true,
                        };
                        fits.then_some((module, f.as_str()))
                    })
                    .collect();
                names.sort();
                names.dedup_by_key(|(m, _)| *m);
                for (module, file) in names {
                    if !out.iter().any(|m| m.name == module) {
                        let mut m = Member { name: module.to_string(), values: Vec::new(), doc: String::new(), location: Some((file.to_string(), 0)) };
                        // `function lib.name()` in the module file gives the doc and value.
                        let sub = Anchor { base: a.base.clone(), path: vec![module.to_string()] };
                        if let Some((d, cx)) = self.defs(&sub, false).into_iter().next() {
                            m.values.push((d.value.clone(), cx.clone()));
                            m.doc = d.doc.clone();
                            m.location = Some((cx.file.clone(), d.offset));
                        }
                        out.push(m);
                    }
                }
            }
        }
        for alias in self.aliases(a, true) {
            self.members(&alias, out, seen);
        }
    }

    /// Other names for the anchor: where `base.path[..k]` is assigned a value,
    /// that value's anchors with the rest of the path (`local C = Core` makes
    /// `C.Functions` also `Core.Functions`). `exact`: `k` may be the whole path.
    fn aliases(&self, a: &Anchor, exact: bool) -> Vec<Anchor> {
        let mut out = Vec::new();
        // `xPlayer.job.` through `---@field job ESXJob`.
        if let Base::Class { name, res, side } = &a.base
            && !a.path.is_empty()
            && (exact || a.path.len() > 1)
        {
            let cx = Cx { file: String::new(), res: *res, side: *side };
            for (ty, fcx) in self.class_field_types(name, *res, &a.path[0], &cx) {
                let mut vals = Vec::new();
                self.resolve_doc(&ty, &fcx, &mut vals);
                for v in vals {
                    if let Val::Anchor(mut inner) = v
                        && inner.path.len() + a.path.len() <= 16
                    {
                        inner.path.extend(a.path[1..].iter().cloned());
                        out.push(inner);
                    }
                }
            }
        }
        let top = if exact { a.path.len() } else { a.path.len().saturating_sub(1) };
        for k in 0..=top {
            if !exact && k == a.path.len() {
                break;
            }
            for (d, cx) in self.defs_at(a, &a.path[..k], false) {
                let mut vals = Vec::new();
                self.resolve(&d.value, &cx, &mut vals);
                for v in vals {
                    if let Val::Anchor(mut inner) = v
                        && inner.path.len() + a.path.len() - k <= 16
                    {
                        inner.path.extend(a.path[k..].iter().cloned());
                        out.push(inner);
                    }
                }
            }
        }
        out
    }

    fn item(&self, m: &Member) -> ApiItem {
        let mut funcs = Vec::new();
        let mut seen = HashSet::new();
        for (r, cx) in &m.values {
            self.funcs_of_ref(r, cx, &mut funcs, &mut seen);
            if !funcs.is_empty() {
                break;
            }
        }
        let f = funcs.first().map(|(f, _)| *f);
        let declared = m.values.iter().find_map(|(r, _)| match r {
            Ref::Doc(t) => Some(t),
            _ => None,
        });
        let doc_fun = declared.and_then(|t| match t {
            DocType::Fun(f) => Some(&**f),
            DocType::Optional(inner) => match &**inner {
                DocType::Fun(f) => Some(&**f),
                _ => None,
            },
            _ => None,
        });
        // `exports('Name', fn)` documents nothing itself: the doc is where the
        // value is defined (`---@deprecated` above `local function fn`).
        let doc = if m.doc.is_empty() {
            m.values
                .iter()
                .find_map(|(r, cx)| {
                    let owner = match r {
                        Ref::Local(d) => Owner::Local(*d),
                        Ref::Global(n) => Owner::Global(n.clone()),
                        _ => return None,
                    };
                    let api = self.api(&cx.file)?;
                    api.defs_of(&owner).iter().find(|d| d.path.is_empty() && !d.doc.is_empty()).map(|d| d.doc.clone())
                })
                .unwrap_or_default()
        } else {
            m.doc.clone()
        };
        let params = f.map(|f| f.params.clone()).or_else(|| {
            doc_fun.map(|d| d.params.iter().filter(|p| p.name != "self").map(|p| p.name.clone()).collect())
        });
        let ty = declared.map(crate::luacats::view).or_else(|| {
            let f = f?;
            let parts: Vec<String> = f
                .params
                .iter()
                .enumerate()
                .map(|(i, p)| match f.param_types.get(i).and_then(Option::as_ref) {
                    Some(t) => format!("{p}: {}", crate::luacats::view(t)),
                    None => p.clone(),
                })
                .collect();
            let rets: Vec<String> = f.returns.iter().map(crate::luacats::view).collect();
            (!f.param_types.is_empty() || !f.returns.is_empty()).then(|| {
                let mut s = format!("fun({})", parts.join(", "));
                if !rets.is_empty() {
                    s.push(':');
                    s.push_str(&rets.join(", "));
                }
                s
            })
        });
        ApiItem {
            name: m.name.clone(),
            params,
            ty,
            method: f.is_some_and(|f| f.method),
            doc,
            location: m.location.as_ref().and_then(|(file, o)| Some((self.entry(file)?.path.clone(), *o))),
            side: FileSide::Unknown,
        }
    }

    /// A global: its definitions in the state.
    fn global_item(&self, cx: &Cx, name: &str) -> Option<ApiItem> {
        let a = Anchor { base: Base::Global { res: cx.res, side: cx.side, name: name.to_string() }, path: Vec::new() };
        let defs = self.defs(&a, false);
        let (d, dcx) = defs.iter().find(|(d, _)| !d.doc.is_empty()).or(defs.first())?;
        let m = Member {
            name: name.to_string(),
            values: defs.iter().map(|(d, c)| (d.value.clone(), c.clone())).collect(),
            doc: d.doc.clone(),
            location: Some((dcx.file.clone(), d.offset)),
        };
        Some(self.item(&m))
    }

    /// The arguments every definition of `root.fields...` takes: the most, and
    /// the fewest it requires (documented, not optional, not nil-able), `self`
    /// of a `:` method counted.
    fn arity(&self, cx: &Cx, root: &Ref, fields: &[String]) -> Arity {
        let mut values: Vec<(Ref, Cx)> = Vec::new();
        let mut docs: Vec<String> = Vec::new();
        match fields.split_last() {
            None => {
                let base = match root {
                    Ref::Global(n) => Base::Global { res: cx.res, side: cx.side, name: n.clone() },
                    Ref::Local(d) => Base::Local { file: cx.file.clone(), decl: *d },
                    _ => return Arity::Unknown,
                };
                for (d, dcx) in self.defs(&Anchor { base, path: Vec::new() }, false) {
                    values.push((d.value.clone(), dcx));
                    docs.push(d.doc.clone());
                }
            }
            Some((last, parent)) => {
                let r = parent.iter().fold(root.clone(), |r, f| Ref::Field(Box::new(r), f.clone()));
                let mut vals = Vec::new();
                self.resolve(&r, cx, &mut vals);
                for v in vals {
                    match v {
                        // A class has `---@field`s besides what code sets.
                        Val::Anchor(Anchor { base: Base::Class { .. }, .. }) => return Arity::Unknown,
                        Val::Anchor(mut a) => {
                            a.path.push(last.clone());
                            for (d, dcx) in self.defs(&a, false) {
                                values.push((d.value.clone(), dcx));
                                docs.push(d.doc.clone());
                            }
                        }
                        // A value known by its doc type only.
                        _ => return Arity::Unknown,
                    }
                }
            }
        }
        if values.is_empty() {
            return if self.budget.get() == 0 { Arity::Unknown } else { Arity::NotFound };
        }
        if docs.iter().any(|d| d.contains("@overload") || d.contains("@type") || d.contains("@vararg")) {
            return Arity::Unknown;
        }
        let (mut max, mut min) = (0, usize::MAX);
        for (r, vcx) in &values {
            // Another resource's API (ox_lib, a core's `Functions`) is seen in part:
            // its other definitions may sit behind a side check or a lazy loader.
            if vcx.res != cx.res {
                return Arity::Unknown;
            }
            let Ref::Func(id) = r else { return Arity::Unknown };
            let Some(f) = self.api(&vcx.file).and_then(|a| a.funcs.get(*id as usize)) else { return Arity::Unknown };
            if f.params.iter().any(|p| p == "...") {
                return Arity::Unknown;
            }
            let shift = usize::from(f.method);
            let required = f
                .param_types
                .iter()
                .enumerate()
                .rposition(|(i, t)| t.as_ref().is_some_and(required_type) && !f.nil_ok.get(i).copied().unwrap_or(false))
                .map_or(0, |i| i + 1);
            max = max.max(f.params.len() + shift);
            min = min.min(required + shift);
        }
        if self.budget.get() == 0 {
            return Arity::Unknown;
        }
        Arity::Known(max, min)
    }

    /// `root.fields...` (a global when `fields` is empty).
    fn member_item(&self, cx: &Cx, root: &Ref, fields: &[String]) -> Option<ApiItem> {
        let Some((last, parent)) = fields.split_last() else {
            let Ref::Global(name) = root else { return None };
            return self.global_item(cx, name);
        };
        let r = parent.iter().fold(root.clone(), |r, f| Ref::Field(Box::new(r), f.clone()));
        let mut vals = Vec::new();
        self.resolve(&r, cx, &mut vals);
        let mut members = Vec::new();
        let mut seen = HashSet::new();
        for v in vals {
            if let Val::Anchor(a) = v {
                self.members(&a, &mut members, &mut seen);
            }
        }
        members.iter().find(|m| m.name == *last).map(|m| self.item(m))
    }

    fn provider(&self, name: &str) -> Option<usize> {
        let lower = name.to_lowercase();
        self.ws.by_name.get(&lower).copied().or_else(|| {
            self.ws.resources.iter().position(|r| r.manifest.provides.iter().any(|p| p.eq_ignore_ascii_case(name)))
        })
    }

    /// Every export of the resource answering for `name`: (name, values, doc,
    /// location, side registered on).
    fn exports_raw(&self, name: &str) -> Option<Rc<RawExports>> {
        let key = name.to_lowercase();
        if let Some(v) = self.exports.borrow().get(&key) {
            return v.clone();
        }
        let v = self.exports_uncached(name).map(Rc::new);
        self.exports.borrow_mut().insert(key, v.clone());
        v
    }

    fn exports_uncached(&self, name: &str) -> Option<RawExports> {
        let id = self.provider(name)?;
        let r = &self.ws.resources[id];
        // Through `provide`, only exports registered under the provided name
        // answer: `exports(...)` registers under the resource's own name.
        let own = r.name.eq_ignore_ascii_case(name);
        let mut members: Vec<Member> = Vec::new();
        let mut sides: Vec<FileSide> = Vec::new();
        let mut complete = !r.escrow && !r.manifest.dynamic && !r.foreign && !self.ws.duplicate_names.contains(&r.name.to_lowercase());
        let add = |members: &mut Vec<Member>, sides: &mut Vec<FileSide>, m: Member, side: FileSide| {
            match members.iter().position(|x| x.name == m.name) {
                Some(i) => {
                    if sides[i] != side {
                        sides[i] = if sides[i] == FileSide::Unknown || side == FileSide::Unknown { FileSide::Unknown } else { FileSide::Shared };
                    }
                    if members[i].doc.is_empty() {
                        members[i].doc = m.doc;
                    }
                    members[i].values.extend(m.values);
                }
                None => {
                    members.push(m);
                    sides.push(side);
                }
            }
        };
        for f in &r.files {
            let (Some(e), Some(api)) = (self.entry(f), self.api(f)) else { continue };
            let side = r.manifest.side(&e.rel);
            if api.dynamic_exports && api.exports_from.is_empty() {
                complete = false;
            }
            let cx0 = Cx { file: f.clone(), res: Some(id), side };
            for callee in &api.dynamic_export_calls {
                let mut funcs = Vec::new();
                self.funcs_of_ref(callee, &cx0, &mut funcs, &mut HashSet::new());
                // A wrapper (or one that cannot be read) registering unknown names.
                if funcs.is_empty() || funcs.iter().any(|(f, _)| f.wrapper.is_some()) {
                    complete = false;
                }
            }
            let cx = Cx { file: f.clone(), res: Some(id), side };
            for x in api.exports.iter().filter(|_| own) {
                let m = Member { name: x.name.clone(), values: vec![(x.value.clone(), cx.clone())], doc: x.doc.clone(), location: Some((f.clone(), x.offset)) };
                add(&mut members, &mut sides, m, side);
            }
            for c in &api.export_calls {
                let mut funcs = Vec::new();
                self.funcs_of_ref(&c.callee, &cx, &mut funcs, &mut HashSet::new());
                let wrapper = funcs.iter().find_map(|(f, _)| f.wrapper.as_ref());
                let derived = wrapper.is_some_and(|w| w.derived);
                let ours = match (funcs.is_empty(), wrapper) {
                    // A wrapper that cannot be read: trust its name.
                    (true, _) => own,
                    (false, Some(w)) => match &w.resource {
                        Some(res) => res.eq_ignore_ascii_case(name),
                        None => own,
                    },
                    (false, None) => false,
                };
                if ours {
                    let mut names = vec![c.name.clone()];
                    if derived {
                        // Both casings of the first letter.
                        let mut chars = c.name.chars();
                        if let Some(first) = chars.next() {
                            let rest: String = chars.collect();
                            for v in [format!("{}{rest}", first.to_uppercase()), format!("{}{rest}", first.to_lowercase())] {
                                if !names.contains(&v) {
                                    names.push(v);
                                }
                            }
                        }
                    }
                    for n in names {
                        let m = Member { name: n, values: vec![(c.value.clone(), cx.clone())], doc: c.doc.clone(), location: Some((f.clone(), c.offset)) };
                        add(&mut members, &mut sides, m, side);
                    }
                }
            }
            for t in api.exports_from.iter().filter(|_| own) {
                let mut vals = Vec::new();
                self.resolve(t, &cx, &mut vals);
                let mut found = Vec::new();
                for v in vals {
                    if let Val::Anchor(a) = v {
                        self.members(&a, &mut found, &mut HashSet::new());
                    }
                }
                if found.is_empty() {
                    complete = false;
                }
                for m in found {
                    add(&mut members, &mut sides, m, side);
                }
            }
        }
        // `export 'Name'` / `server_export 'Name'`: global functions.
        let none = Vec::new();
        let (client, server) = if own { (&r.manifest.client_exports, &r.manifest.server_exports) } else { (&none, &none) };
        for (list, side) in [(client, FileSide::Client), (server, FileSide::Server)] {
            for n in list {
                let cx = Cx { file: String::new(), res: Some(id), side };
                let anchor = Anchor { base: Base::Global { res: Some(id), side, name: n.clone() }, path: Vec::new() };
                let def = self.defs(&anchor, false).into_iter().next();
                let m = Member {
                    name: n.clone(),
                    values: vec![(Ref::Global(n.clone()), cx)],
                    doc: def.as_ref().map(|(d, _)| d.doc.clone()).unwrap_or_default(),
                    location: def.as_ref().map(|(d, cx)| (cx.file.clone(), d.offset)),
                };
                add(&mut members, &mut sides, m, side);
            }
        }
        // Exports of the JavaScript scripts (no parameters known).
        for (n, side) in r.js_exports.iter().filter(|_| own) {
            let m = Member { name: n.clone(), values: Vec::new(), doc: String::new(), location: None };
            add(&mut members, &mut sides, m, *side);
        }
        // Exports registered by hand through the export event.
        let prefixes = [format!("__cfx_export_{name}_"), format!("__cfx_export_{}_", r.name)];
        if self.ws.export_events.keys().any(|s| !s.ends_with("%s") && prefixes.iter().any(|p| s.starts_with(p.as_str()))) {
            complete = false;
        }
        Some((id, members, sides, complete))
    }

    fn export_values(&self, res: &str, name: &str, side: FileSide) -> Vec<(Ref, Cx)> {
        let Some(raw) = self.exports_raw(res) else { return Vec::new() };
        let (_, members, sides, _) = &*raw;
        // Per value: an export registered on both sides has a value for each.
        members
            .iter()
            .zip(sides)
            .filter(|(m, s)| m.name == name && side_sees(side, **s))
            .flat_map(|(m, _)| m.values.iter().cloned())
            .filter(|(_, cx)| side_sees(side, cx.side))
            // A shared file's value, called from one side, lives in that side's
            // state: `ESX` returned by a shared export has only that side's fields.
            .map(|(r, mut cx)| {
                if matches!(cx.side, FileSide::Shared | FileSide::Unknown) && matches!(side, FileSide::Client | FileSide::Server) {
                    cx.side = side;
                }
                (r, cx)
            })
            .collect()
    }
}

/// Edit distance, for "did you mean" (short names only).
fn distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.to_lowercase().chars().collect(), b.to_lowercase().chars().collect());
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut prev = row[0];
        row[0] = i;
        for j in 1..=b.len() {
            let cur = row[j];
            row[j] = (prev + usize::from(a[i - 1] != b[j - 1])).min(row[j] + 1).min(row[j - 1] + 1);
            prev = cur;
        }
    }
    row[b.len()]
}

fn side_name(s: FileSide) -> &'static str {
    match s {
        FileSide::Client => "client",
        FileSide::Server => "server",
        FileSide::Shared => "shared",
        FileSide::Unknown => "unknown",
    }
}

impl Workspace {
    /// Diagnostics that need the index: exports a resource does not have or
    /// registers on the other side, and uses of `---@deprecated` definitions.
    pub(crate) fn api_lints(&self, path: &Path, parse: &crate::Parse, text: &str, names: &crate::Names, config: &crate::lint::LintConfig) -> Vec<crate::lint::Lint> {
        use cfx_lua_syntax::SyntaxKind::*;
        let mut out = Vec::new();
        let (k, cx) = self.query_cx(path);
        let overlay = api::file_api(parse, text, names);
        // The file's own `---@deprecated` names (the index may hold an older copy).
        let own_deprecated: HashSet<&str> = overlay.deprecated_names().collect();
        let q = Q::new(self, Some((k.as_str(), &overlay)));
        let mut push = |code: &'static str, range: crate::TextRange, message: String, tag: Option<crate::lint::Tag>| {
            if let Some(l) = crate::lint::make(code, range, message, tag, config) {
                out.push(l);
            }
        };
        let deprecated_msg = |name: &str, advice: &str| {
            if advice.is_empty() { format!("`{name}` is deprecated.") } else { format!("`{name}` is deprecated: {advice}") }
        };
        let mut arities = ArityCx { known: HashMap::new(), written: HashSet::new() };
        arities.written = names
            .refs
            .iter()
            .filter(|r| matches!(r.access, crate::names::Access::Write | crate::names::Access::ReadWrite))
            .filter_map(|r| match r.target {
                crate::names::Target::Decl(d) => Some(d),
                crate::names::Target::Global => None,
            })
            .collect();
        for n in parse.syntax().descendants() {
            if n.kind() == CALL_STAT
                && let Some(call) = n.first_child()
                && self.no_discard(&q, &cx, text, parse, names, &call)
            {
                push("discard-returns", call.text_range(), "The return values of this function cannot be discarded.".to_string(), None);
            }
            if matches!(n.kind(), CALL_EXPR | METHOD_CALL_EXPR) {
                for (code, range, message) in self.arity_lints(&q, &cx, text, names, &n, &mut arities) {
                    push(code, range, message, None);
                }
            }
            match n.kind() {
                CALL_EXPR => {
                    // `TriggerEvent('QBCore:GetObject', cb)` where nothing answers:
                    // qbx_core has no handler, qb-core has one.
                    let Some(callee) = n.first_child().filter(|c| c.kind() == NAME_EXPR) else { continue };
                    let callee_name = &text[callee.text_range()];
                    if matches!(callee_name, "RegisterNetEvent" | "RegisterServerEvent") && cx.side == FileSide::Server {
                        if let Some(l) = self.net_event_local_only(&n, text, &cx, &overlay) {
                            push("fivem/net-event-same-side", l.0, l.1, None);
                        }
                        continue;
                    }
                    if callee_name != "TriggerEvent" {
                        continue;
                    }
                    let first = n.children().find(|c| c.kind() == ARG_LIST).and_then(|a| a.children().next());
                    let Some(first) = first.filter(|f| f.kind() == LITERAL_EXPR) else { continue };
                    let event = crate::util::unquote(text[first.text_range()].trim());
                    if event == Some("esx:getSharedObject") {
                        // ESX Legacy (1.12.4, 1.13.4) still answers the event from
                        // shared/main.lua, next to the export; 1.12.4 prints a
                        // warning on every use.
                        q.budget.set(BUDGET);
                        let exported = q.exports_raw("es_extended").is_some_and(|raw| raw.1.iter().any(|m| m.name == "getSharedObject"));
                        if exported {
                            push(
                                "esx/shared-object-event",
                                n.text_range(),
                                "ESX Legacy imports ESX with `exports.es_extended:getSharedObject()` or `@es_extended/imports.lua` in the manifest; the `esx:getSharedObject` event is kept for old scripts (1.12 prints a warning on every use).".to_string(),
                                None,
                            );
                        }
                        continue;
                    }
                    if event != Some("QBCore:GetObject") {
                        continue;
                    }
                    q.budget.set(BUDGET);
                    let known = q.exports_raw("qb-core").is_some_and(|raw| raw.3);
                    let handled = self.files.values().any(|e| e.api.events_handled.iter().any(|(e, _)| e == "QBCore:GetObject"))
                        || overlay.events_handled.iter().any(|(e, _)| e == "QBCore:GetObject");
                    if known && !handled {
                        let provider = self.provider_name("qb-core").unwrap_or_else(|| "qb-core".into());
                        push(
                            "qb/core-object-event",
                            n.text_range(),
                            format!("No resource handles `QBCore:GetObject` (`qb-core` is `{provider}` here), so the callback never runs: use `exports['qb-core']:GetCoreObject()`."),
                            None,
                        );
                    }
                }
                METHOD_CALL_EXPR => {
                    let (Some(object), Some(m)) = (n.first_child(), api::method_name(&n)) else { continue };
                    if let Some(res) = api::exports_resource(text, names, &object) {
                        q.budget.set(BUDGET);
                        let Some(raw) = q.exports_raw(&res) else { continue };
                        let (id, members, sides, complete) = &*raw;
                        let name = m.text();
                        let found: Vec<(&Member, FileSide)> =
                            members.iter().zip(sides.iter().copied()).filter(|(x, _)| x.name == name).collect();
                        if let Some((mem, _)) = found.iter().find(|(_, s)| side_sees(cx.side, *s)) {
                            if let Some(d) = q.item(mem).deprecated() {
                                push("deprecated", m.text_range(), deprecated_msg(name, d), Some(crate::lint::Tag::Deprecated));
                            }
                            continue;
                        }
                        if !complete {
                            continue;
                        }
                        let provider = &self.resources[*id].name;
                        let via = if provider.eq_ignore_ascii_case(&res) { String::new() } else { format!(" (`{res}` is provided by `{provider}`)") };
                        if let Some((_, s)) = found.first() {
                            if matches!(cx.side, FileSide::Client | FileSide::Server) && matches!(s, FileSide::Client | FileSide::Server) {
                                push(
                                    "fivem/export-wrong-side",
                                    m.text_range(),
                                    format!("`{name}` is a {} export of `{res}`{via}: a {} script cannot call it (each side has its own exports).", side_name(*s), side_name(cx.side)),
                                    None,
                                );
                            }
                            continue;
                        }
                        let close = members
                            .iter()
                            .zip(sides.iter().copied())
                            .filter(|(_, s)| side_sees(cx.side, *s))
                            .map(|(x, _)| (distance(&x.name, name), &x.name))
                            .filter(|(d, _)| *d <= 2)
                            .min();
                        let hint = close.map(|(_, c)| format!(" Did you mean `{c}`?")).unwrap_or_default();
                        push("fivem/unknown-export", m.text_range(), format!("`{res}` has no export `{name}`{via}.{hint}"), None);
                        continue;
                    }
                    if (self.deprecated.contains_key(m.text()) || own_deprecated.contains(m.text()))
                        && let Some((root, mut fields)) = crate::assist::chain(text, names, &object)
                    {
                        fields.push(m.text().to_string());
                        q.budget.set(BUDGET);
                        if let Some(item) = q.member_item(&cx, &root, &fields)
                            && let Some(d) = item.deprecated()
                        {
                            // As LuaLS marks it: `T:f`.
                            let range = crate::TextRange::new(object.text_range().start(), m.text_range().end());
                            push("deprecated", range, deprecated_msg(m.text(), d), Some(crate::lint::Tag::Deprecated));
                        }
                    }
                }
                INDEX_EXPR => {
                    let Some(f) = crate::lint::field_name(&n) else { continue };
                    if !(self.deprecated.contains_key(f.text()) || own_deprecated.contains(f.text())) || n.parent().is_some_and(|p| p.kind() == VAR_LIST) {
                        continue;
                    }
                    let Some((root, fields)) = crate::assist::chain(text, names, &n) else { continue };
                    q.budget.set(BUDGET);
                    if let Some(item) = q.member_item(&cx, &root, &fields)
                        && let Some(d) = item.deprecated()
                    {
                        // As LuaLS marks it: `T.x`.
                        let range = crate::TextRange::new(n.text_range().start(), f.text_range().end());
                        push("deprecated", range, deprecated_msg(f.text(), d), Some(crate::lint::Tag::Deprecated));
                    }
                }
                NAME_EXPR => {
                    let Some(t) = n.first_token() else { continue };
                    // `cfx.lualsCompat`: a library global of another Lua version
                    // (`unpack` is 5.1; CfxLua keeps it).
                    if config.luals_compat
                        && let Some(v) = crate::library::other_version(t.text())
                        && n.parent().is_none_or(|p| p.kind() != VAR_LIST)
                        && matches!(crate::assist::chain(text, names, &n), Some((Ref::Global(_), f)) if f.is_empty())
                    {
                        push("deprecated", t.text_range(), format!("Deprecated (defined in Lua {v})."), Some(crate::lint::Tag::Deprecated));
                        continue;
                    }
                    if !(self.deprecated.contains_key(t.text()) || own_deprecated.contains(t.text())) || n.parent().is_some_and(|p| p.kind() == VAR_LIST) {
                        continue;
                    }
                    let Some((Ref::Global(name), fields)) = crate::assist::chain(text, names, &n) else { continue };
                    q.budget.set(BUDGET);
                    if fields.is_empty()
                        && let Some(item) = q.global_item(&cx, &name)
                        && let Some(d) = item.deprecated()
                    {
                        push("deprecated", t.text_range(), deprecated_msg(&name, d), Some(crate::lint::Tag::Deprecated));
                    }
                }
                _ => {}
            }
        }
        // An escrow file of the resource or of one it depends on, or an import
        // outside the workspace, may declare any class.
        if text.contains("---") && !self.doc_types_hidden(path) {
            for (range, name, parent) in self.undefined_doc_names(&parse.syntax(), text, &overlay) {
                if parent {
                    push("undefined-doc-class", range, format!("Undefined class `{name}`."), None);
                    continue;
                }
                let lua = match name.as_str() {
                    "int" | "long" => Some("integer"),
                    "float" | "double" => Some("number"),
                    "bool" => Some("boolean"),
                    "str" | "char" => Some("string"),
                    "object" | "array" | "list" | "dict" | "map" => Some("table"),
                    "func" | "callback" | "delegate" => Some("function"),
                    _ => None,
                };
                let message = match lua {
                    Some(l) => format!("Undefined type or alias `{name}` (Lua's is `{l}`)."),
                    None => format!("Undefined type or alias `{name}`."),
                };
                push("undefined-doc-name", range, message, None);
            }
        }
        if text.contains("require") {
            for range in self.different_requires(&q, &cx, k.as_str(), parse, text, names) {
                push("different-requires", range, "The same file is required with different names.".to_string(), None);
            }
        }
        if text.contains("@class") {
            for range in self.circular_classes(&parse.syntax(), text, &q, cx.res) {
                push("circle-doc-class", range, "Circularly inherited classes.".to_string(), None);
            }
        }
        if text.contains("@class") || text.contains("---@type") || text.contains("@enum") {
            // undefined-field and inject-field: only for classes no other file declares.
            let types = crate::types::FileTypes::from_root(parse.syntax(), names, text).with_config(config);
            let found = types.undefined_fields(&parse.syntax());
            let injected = types.inject_fields();
            // Field names other files of the resource write (`AdminPanel.schema = x`).
            let elsewhere: HashSet<&str> = if found.is_empty() && injected.is_empty() {
                HashSet::new()
            } else {
                self.files
                    .iter()
                    .filter(|(f, e)| e.resource == cx.res && f.as_str() != k.as_str())
                    .filter_map(|(f, _)| q.api(f))
                    .flat_map(|a| a.defs.iter().filter_map(|d| d.path.last().map(String::as_str)))
                    .collect()
            };
            for (range, chain) in found {
                if !elsewhere.contains(&text[range]) && chain.iter().all(|c| q.class_files(c, cx.res).iter().all(|f| *f == k.as_str())) {
                    push("undefined-field", range, format!("Undefined field `{}`.", &text[range]), None);
                }
            }
            for (range, chain, message) in injected {
                if !elsewhere.contains(&text[range]) && chain.iter().all(|c| q.class_files(c, cx.res).iter().all(|f| *f == k.as_str())) {
                    push("inject-field", range, message, None);
                }
            }
            for (range, owner, message) in types.invisible_fields(&config.private_names, &config.protected_names, config.lua_name_patterns) {
                if q.class_files(&owner, cx.res).iter().all(|f| *f == k.as_str()) {
                    push("invisible", range, message, None);
                }
            }
        }
        out
    }

    /// different-requires: `require 'x'` naming a file that this file or
    /// another file of the resource requires under another name (`a.b` and
    /// `a/b`, `@res.a` from inside `res`).
    fn different_requires(&self, q: &Q, cx: &Cx, k: &str, parse: &crate::Parse, text: &str, names: &crate::Names) -> Vec<crate::TextRange> {
        use cfx_lua_syntax::SyntaxKind::*;
        let mut out = Vec::new();
        let mut mine: Vec<(crate::TextRange, String)> = Vec::new();
        for call in parse.syntax().descendants().filter(|c| c.kind() == CALL_EXPR) {
            let Some(callee) = call.first_child().filter(|c| c.kind() == NAME_EXPR && &text[c.text_range()] == "require") else { continue };
            if callee.first_token().is_some_and(|t| names.refs.iter().any(|r| r.range == t.text_range() && r.target != crate::names::Target::Global)) {
                continue;
            }
            // `require('x')` holds a literal expression, `require 'x'` the string itself.
            let Some(args) = call.children().find(|c| c.kind() == ARG_LIST) else { continue };
            let tok = args.children_with_tokens().find_map(|e| match e {
                rowan::NodeOrToken::Node(n) => n.first_token().filter(|_| n.kind() == LITERAL_EXPR),
                rowan::NodeOrToken::Token(t) => (t.kind() == STRING).then_some(t),
            });
            let Some(tok) = tok else { continue };
            let Some(lit) = crate::util::unquote(tok.text()).map(str::to_string) else { continue };
            mine.push((tok.text_range(), lit));
        }
        if mine.is_empty() {
            return out;
        }
        fn walk(r: &Ref, out: &mut Vec<String>) {
            match r {
                Ref::Require(m) => out.push(m.clone()),
                Ref::Field(b, _) | Ref::Call(b) => walk(b, out),
                _ => {}
            }
        }
        // The names other files of the resource use, by the file they reach.
        let mut seen: HashMap<&str, String> = HashMap::new();
        let others = cx.res.map(|r| self.resources[r].files.as_slice()).unwrap_or_default();
        for f in others.iter().filter(|f| f.as_str() != k) {
            let Some(api) = q.api(f) else { continue };
            let mut lits = Vec::new();
            for d in &api.defs {
                walk(&d.value, &mut lits);
            }
            if let Some(r) = &api.ret {
                walk(r, &mut lits);
            }
            let fcx = Cx { file: f.clone(), res: cx.res, side: cx.side };
            for lit in lits {
                if let Some(target) = q.module(&lit, &fcx) {
                    seen.entry(target).or_insert(lit);
                }
            }
        }
        for (range, lit) in mine {
            let Some(target) = q.module(&lit, cx) else { continue };
            match seen.get(target) {
                Some(other) if *other != lit => out.push(range),
                Some(_) => {}
                None => {
                    seen.insert(target, lit);
                }
            }
        }
        out
    }

    /// redundant-parameter and missing-parameter for one call, against every
    /// definition of the function it calls (a call on a `:` method counts
    /// `self`). Silent unless every definition is a function of the workspace
    /// without `...`, `---@overload` or `---@type`; the Lua library's own
    /// functions are checked for extra arguments only.
    fn arity_lints(
        &self,
        q: &Q,
        cx: &Cx,
        text: &str,
        names: &crate::Names,
        call: &cfx_lua_syntax::SyntaxNode,
        arities: &mut ArityCx,
    ) -> Vec<(&'static str, crate::TextRange, String)> {
        use cfx_lua_syntax::SyntaxKind::*;
        let mut out = Vec::new();
        let Some(callee) = call.first_child() else { return out };
        let Some(args) = call.children().find(|c| c.kind() == ARG_LIST) else { return out };
        // `f'x'` and `f{...}` pass one argument.
        let parens = args.first_token().is_some_and(|t| t.kind() == L_PAREN);
        let values: Vec<cfx_lua_syntax::SyntaxNode> = args.children().collect();
        let ranges: Vec<crate::TextRange> = if parens { values.iter().map(|v| v.text_range()).collect() } else { vec![args.text_range()] };
        let method_call = call.kind() == METHOD_CALL_EXPR;
        let target = if method_call {
            if api::exports_resource(text, names, &callee).is_some() {
                return out;
            }
            let Some(m) = api::method_name(call) else { return out };
            crate::assist::chain(text, names, &callee).map(|(root, mut fields)| {
                fields.push(m.text().to_string());
                (root, fields)
            })
        } else {
            crate::assist::chain(text, names, &callee)
        };
        let Some((root, fields)) = target else { return out };
        // A local called by name: a function it was declared with, never assigned
        // again (`local ok, f = pcall(...)` then `f = function() end` is not known).
        if let (Ref::Local(d), true) = (&root, fields.is_empty()) {
            let decl = &names.decls[*d as usize];
            let declared = matches!(decl.kind, crate::names::DeclKind::LocalFunction) || decl.function.is_some();
            if !declared || arities.written.contains(&(*d as usize)) {
                return out;
            }
        }
        let key = (root, fields, method_call);
        let known = match arities.known.get(&key) {
            Some(k) => *k,
            None => {
                q.budget.set(BUDGET);
                // The library's definition of a global counts too (`print = function() end`
                // does not make `print(a)` wrong: the library's `print` takes `...`).
                let lib = if method_call { None } else { library_arity(names, &callee) };
                let k = match (q.arity(cx, &key.0, &key.1), lib) {
                    (Arity::Unknown, _) | (_, Some((None, _))) => None,
                    (Arity::Known(max, min), None) => Some((max, min)),
                    (Arity::Known(max, _), Some((Some(n), _))) => Some((max.max(n), 0)),
                    (Arity::NotFound, Some((Some(n), crate::library::Origin::Lua))) => Some((n, 0)),
                    (Arity::NotFound, _) => None,
                };
                arities.known.insert(key, k);
                k
            }
        };
        let Some((max, min)) = known else { return out };
        let shift = usize::from(method_call);
        let count = ranges.len() + shift;
        if count > max {
            let msg = format!("This function expects a maximum of {max} argument(s) but instead it is receiving {count}.");
            for r in ranges.iter().skip(max.saturating_sub(shift)) {
                out.push(("redundant-parameter", *r, msg.clone()));
            }
        }
        let open = parens && values.last().is_some_and(|v| matches!(v.kind(), CALL_EXPR | METHOD_CALL_EXPR | VARARG_EXPR));
        if count < min && !open {
            let msg = format!("This function requires {min} argument(s) but instead it is receiving {count}.");
            out.push(("missing-parameter", call.text_range(), msg));
        }
        out
    }

    /// The function a call calls is `---@nodiscard`: a local function of the
    /// file, a function of the workspace, or one of the library.
    fn no_discard(&self, q: &Q, cx: &Cx, text: &str, parse: &crate::Parse, names: &crate::Names, call: &cfx_lua_syntax::SyntaxNode) -> bool {
        use cfx_lua_syntax::SyntaxKind::*;
        let Some(callee) = call.first_child() else { return false };
        let target = match call.kind() {
            METHOD_CALL_EXPR => {
                if api::exports_resource(text, names, &callee).is_some() {
                    return false;
                }
                let Some(m) = api::method_name(call) else { return false };
                crate::assist::chain(text, names, &callee).map(|(root, mut fields)| {
                    fields.push(m.text().to_string());
                    (root, fields)
                })
            }
            _ => crate::assist::chain(text, names, &callee),
        };
        let Some((root, fields)) = target else { return false };
        if let (Ref::Local(d), true) = (&root, fields.is_empty()) {
            let decl = &names.decls[*d as usize];
            let stat = parse
                .syntax()
                .token_at_offset(decl.range.start())
                .right_biased()
                .and_then(|t| t.parent_ancestors().find(|a| matches!(a.kind(), LOCAL_FUNCTION_STAT | LOCAL_STAT)));
            return stat.is_some_and(|s| api::leading_doc(text, s.text_range()).lines().any(|l| l.trim() == "@nodiscard"));
        }
        if call.kind() == CALL_EXPR
            && let Some(path) = crate::assist::expr_path(names, &callee)
        {
            let lib = crate::library::library();
            if let Some(item) = lib.get(&path).iter().find(|i| i.function) {
                return lib.doc(item).lines().any(|l| l.trim() == "@nodiscard");
            }
        }
        q.budget.set(BUDGET);
        q.member_item(cx, &root, &fields).is_some_and(|i| i.doc.lines().any(|l| l.trim() == "@nodiscard"))
    }

    /// `---@class A : B` lines of the file whose class, through its parents,
    /// inherits from itself.
    fn circular_classes(&self, root: &cfx_lua_syntax::SyntaxNode, text: &str, q: &Q, res: Option<usize>) -> Vec<crate::TextRange> {
        use crate::luacats::{DocType, Tag};
        let mut out = Vec::new();
        for g in crate::lint::code_doc_groups(root, text) {
            for t in &g.tags {
                let Tag::Class { name, parents, .. } = &t.tag else { continue };
                if parents.is_empty() {
                    continue;
                }
                let direct: Vec<String> = parents
                    .iter()
                    .filter_map(|p| match p {
                        DocType::Name(n) | DocType::Generic(n, _) => Some(n.clone()),
                        _ => None,
                    })
                    .collect();
                // Walk the parents; at most 64 classes.
                let mut seen: HashSet<String> = HashSet::new();
                let mut stack = direct;
                let mut circular = false;
                while let Some(c) = stack.pop() {
                    if c == *name {
                        circular = true;
                        break;
                    }
                    if seen.len() > 64 || !seen.insert(c.clone()) {
                        continue;
                    }
                    for (def, _) in q.class_defs(&c, res) {
                        stack.extend(def.parents.iter().cloned());
                    }
                }
                if circular && let Some(n) = t.name {
                    let line = &text[crate::TextRange::new(n.start(), t.range.end())];
                    out.push(crate::TextRange::at(n.start(), crate::TextSize::of(line.trim_end())));
                }
            }
        }
        out
    }

    /// Type names in the file's doc comments that no `---@class`, `---@alias`
    /// or `---@enum` of the workspace or the library declares, that are not
    /// builtin and not a generic the file declares.
    /// The third value: the name is a class's parent (`undefined-doc-class`).
    fn undefined_doc_names(&self, root: &cfx_lua_syntax::SyntaxNode, text: &str, overlay: &api::FileApi) -> Vec<(crate::TextRange, String, bool)> {
        use crate::luacats::{DocType, Key, Tag};
        fn walk<'t>(t: &'t DocType, out: &mut Vec<&'t str>) {
            match t {
                DocType::Name(n) => out.push(n),
                DocType::Generic(n, args) => {
                    out.push(n);
                    args.iter().for_each(|a| walk(a, out));
                }
                DocType::Array(i) | DocType::Optional(i) => walk(i, out),
                DocType::Union(ps) | DocType::Tuple(ps) => ps.iter().for_each(|p| walk(p, out)),
                DocType::Fun(f) => {
                    f.params.iter().filter_map(|p| p.ty.as_ref()).for_each(|p| walk(p, out));
                    f.rets.iter().for_each(|r| walk(&r.ty, out));
                }
                DocType::Table(fields) => {
                    for f in fields {
                        if let Key::Index(k) = &f.key {
                            walk(k, out);
                        }
                        walk(&f.ty, out);
                    }
                }
                _ => {}
            }
        }
        const BUILTIN: &[&str] = &[
            "nil", "any", "unknown", "never", "void", "boolean", "true", "false", "number", "integer", "string", "table", "function", "thread",
            "userdata", "lightuserdata", "self", "...", "fun",
        ];
        let tags: Vec<crate::luacats::DocTag> = crate::lint::code_doc_groups(root, text).into_iter().flat_map(|g| g.tags).collect();
        let mut generics: HashSet<&str> = HashSet::new();
        for t in &tags {
            match &t.tag {
                Tag::Generic { names } => generics.extend(names.iter().map(|(n, _)| n.as_str())),
                Tag::Class { generics: g, .. } | Tag::Alias { generics: g, .. } => generics.extend(g.iter().map(String::as_str)),
                _ => {}
            }
        }
        let lib = crate::library::doc_names();
        // The text as it is now: it may not be the one the index read.
        let own: HashSet<&str> = overlay.classes.iter().map(|c| c.name.as_str()).chain(overlay.doc_names.iter().map(String::as_str)).collect();
        let mut out = Vec::new();
        for t in &tags {
            let mut used = Vec::new();
            let parent = matches!(t.tag, Tag::Class { .. });
            match &t.tag {
                Tag::Class { parents, .. } => parents.iter().for_each(|p| walk(p, &mut used)),
                Tag::Field { key, ty, .. } => {
                    if let Key::Index(k) = key {
                        walk(k, &mut used);
                    }
                    walk(ty, &mut used);
                }
                Tag::Param { ty: Some(ty), .. } | Tag::AliasValue { ty, .. } | Tag::Overload(ty) | Tag::Vararg(ty) => walk(ty, &mut used),
                Tag::Alias { ty: Some(ty), .. } => walk(ty, &mut used),
                Tag::Return { rets, .. } => rets.iter().for_each(|r| walk(&r.ty, &mut used)),
                Tag::Type { tys } => tys.iter().for_each(|ty| walk(ty, &mut used)),
                Tag::Cast { ops, .. } => ops.iter().for_each(|(_, ty)| walk(ty, &mut used)),
                _ => {}
            }
            for n in used {
                if BUILTIN.contains(&n) || generics.contains(n) || own.contains(n) || self.classes.contains_key(n) || self.doc_names.contains_key(n) || lib.contains(n) {
                    continue;
                }
                // The name's place in the line.
                let line = &text[t.range];
                let found = line.match_indices(n).find(|(i, _)| {
                    let before = line[..*i].chars().next_back();
                    let after = line[i + n.len()..].chars().next();
                    let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | '.'));
                    !word(before) && !word(after)
                });
                let range = found.map_or(t.range, |(i, _)| {
                    crate::TextRange::at(t.range.start() + crate::TextSize::from(i as u32), crate::TextSize::of(n))
                });
                if !out.iter().any(|(r, _, _)| *r == range) {
                    out.push((range, n.to_string(), parent));
                }
            }
        }
        out
    }

    /// `RegisterNetEvent('x', ...)` on the server for an event that only server
    /// code fires (`TriggerEvent`): no client in the workspace sends it, yet any
    /// client may. Silent when something else might send it: the name in another
    /// string (a wrapper's argument, a table of names), an escrow file in the
    /// resource or in one named like the event's prefix.
    fn net_event_local_only(&self, call: &cfx_lua_syntax::SyntaxNode, text: &str, cx: &Cx, overlay: &api::FileApi) -> Option<(crate::TextRange, String)> {
        use cfx_lua_syntax::SyntaxKind::*;
        let first = call.children().find(|c| c.kind() == ARG_LIST)?.children().next().filter(|f| f.kind() == LITERAL_EXPR)?;
        let name = crate::util::unquote(text[first.text_range()].trim())?;
        // The stored summary of this file is replaced by the current one.
        let stored = self.files.get(cx.file.as_str()).map(|e| &e.api);
        let count = |map: &HashMap<String, usize>, own: &dyn Fn(&api::FileApi) -> bool| {
            let total = map.get(name).copied().unwrap_or(0);
            let before = usize::from(stored.is_some_and(own));
            total - before.min(total) + usize::from(own(overlay))
        };
        let server_bound = count(&self.server_bound, &|a| a.server_bound.iter().any(|n| n == name));
        let fired = count(&self.fired_locally, &|a| a.fired_locally.iter().any(|n| n == name));
        let strings = count(&self.event_strings, &|a| a.event_strings.iter().any(|n| n == name));
        if server_bound > 0 || fired == 0 || strings > 0 {
            return None;
        }
        let escrow = |res: &str| self.by_name.get(&res.to_lowercase()).is_some_and(|&id| self.resources[id].escrow);
        if cx.res.is_some_and(|id| self.resources[id].escrow) || name.split(':').next().is_some_and(escrow) {
            return None;
        }
        Some((
            first.text_range(),
            format!("Only server code fires `{name}` (`TriggerEvent`), but `RegisterNetEvent` lets every client send it too: use `AddEventHandler` unless clients should."),
        ))
    }

    /// The name of the resource answering for `name`.
    fn provider_name(&self, name: &str) -> Option<String> {
        let q = Q::new(self, None);
        q.provider(name).map(|id| self.resources[id].name.clone())
    }

    /// The declared types a value has (`xPlayer`, `vector3`,
    /// `fun(id: number):xPlayer?`), as the editor shows them; empty when none
    /// is known.
    pub fn type_views(&self, file: &Path, overlay: Option<&api::FileApi>, r: &Ref) -> Vec<String> {
        let (k, cx) = self.query_cx(file);
        let q = Q::new(self, overlay.map(|o| (k.as_str(), o)));
        let mut out = Vec::new();
        q.type_views(r, &cx, 0, &mut out);
        out
    }

    fn query_cx(&self, file: &Path) -> (String, Cx) {
        let k = key(file);
        let q = Q::new(self, None);
        let cx = q.cx_of(&k, FileSide::Unknown);
        (k, cx)
    }

    /// The fields of `root.fields...` seen from a file. `root` is a `Ref` of the
    /// file (`Ref::Global`, `Ref::Local` by declaration index); `overlay` is the
    /// file's current summary when the stored one may be older.
    pub fn api_members(&self, file: &Path, overlay: Option<&api::FileApi>, root: &Ref, fields: &[String]) -> Vec<ApiItem> {
        let (k, cx) = self.query_cx(file);
        let q = Q::new(self, overlay.map(|o| (k.as_str(), o)));
        let mut vals = Vec::new();
        let r = fields.iter().fold(root.clone(), |r, f| Ref::Field(Box::new(r), f.clone()));
        q.resolve(&r, &cx, &mut vals);
        let mut members = Vec::new();
        let mut seen = HashSet::new();
        for v in vals {
            if let Val::Anchor(a) = v {
                q.members(&a, &mut members, &mut seen);
            }
        }
        members.sort_by(|a, b| a.name.cmp(&b.name));
        members.iter().map(|m| q.item(m)).collect()
    }

    /// One field (`fields` not empty) or global, seen from a file.
    pub fn api_item(&self, file: &Path, overlay: Option<&api::FileApi>, root: &Ref, fields: &[String]) -> Option<ApiItem> {
        let (k, cx) = self.query_cx(file);
        let q = Q::new(self, overlay.map(|o| (k.as_str(), o)));
        q.member_item(&cx, root, fields)
    }

    /// The exports of the resource answering for `resource`, seen from a file
    /// (its side picks which exports it can call). `None`: no such resource in
    /// the workspace.
    pub fn exports_of(&self, file: &Path, resource: &str) -> Option<Exports> {
        let (_, cx) = self.query_cx(file);
        self.exports_on(resource, cx.side)
    }

    /// One export, seen from a file (either side; `side` tells which).
    pub fn export_item(&self, file: &Path, resource: &str, name: &str) -> Option<ApiItem> {
        let q = Q::new(self, None);
        let raw = q.exports_raw(resource)?;
        let (_, members, sides, _) = &*raw;
        let (_, cx) = self.query_cx(file);
        let mut found: Vec<(&Member, FileSide)> =
            members.iter().zip(sides.iter().copied()).filter(|(m, _)| m.name == name).collect();
        found.sort_by_key(|(_, s)| !side_sees(cx.side, *s));
        let (m, s) = found.first()?;
        let mut item = q.item(m);
        item.side = *s;
        Some(item)
    }

    pub fn exports_on(&self, resource: &str, side: FileSide) -> Option<Exports> {
        let q = Q::new(self, None);
        let raw = q.exports_raw(resource)?;
        let (id, members, sides, complete) = &*raw;
        let mut out = Exports { resource: self.resources[*id].name.clone(), complete: *complete, ..Exports::default() };
        for (m, s) in members.iter().zip(sides) {
            let mut item = q.item(m);
            item.side = *s;
            if side_sees(side, *s) { out.items.push(item) } else { out.other_side.push(item) }
        }
        out.items.sort_by(|a, b| a.name.cmp(&b.name));
        out.other_side.sort_by(|a, b| a.name.cmp(&b.name));
        Some(out)
    }

    /// Resource names with exports (and the names they `provide`), for
    /// `exports.` completion.
    pub fn export_resources(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for r in &self.resources {
            let has = r.files.iter().any(|f| {
                self.files.get(f).is_some_and(|e| !e.api.exports.is_empty() || !e.api.export_calls.is_empty() || !e.api.exports_from.is_empty())
            }) || !r.manifest.client_exports.is_empty()
                || !r.manifest.server_exports.is_empty();
            if has {
                out.push(r.name.clone());
                out.extend(r.manifest.provides.iter().cloned());
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// Whether some file marks a definition with this name `---@deprecated`.
    pub fn has_deprecated(&self, name: &str) -> bool {
        self.deprecated.contains_key(name)
    }

    /// The members of a Lua expression evaluated at the end of a file
    /// (`QBCore.Functions`, `lib.callback`, `exports.qbx_core:GetCoreObject()`).
    /// `None`: the expression does not parse or resolves to nothing known.
    pub fn members_of_expression(&self, file: &Path, file_text: &str, expression: &str) -> Option<Vec<ApiItem>> {
        const NAME: &str = "__cfx_lua_ls_query";
        let expression = expression.trim().trim_end_matches(['.', ':']);
        if expression.is_empty() || expression.contains('\n') {
            return None;
        }
        let text = format!("{file_text}\nlocal {NAME} = {expression}\n");
        let p = crate::parse(&text);
        if !p.errors().is_empty() && crate::parse(file_text).errors().len() != p.errors().len() {
            return None;
        }
        let n = crate::names(&p, &text);
        let decl = n.decls.iter().rposition(|d| d.name == NAME)?;
        let overlay = api::file_api(&p, &text, &n);
        let items = self.api_members(file, Some(&overlay), &Ref::Local(decl as u32), &[]);
        (!items.is_empty()).then_some(items)
    }

    /// Globals, their fields, file-level local functions, exports and event
    /// handlers whose name matches `query` (fuzzy), best matches first.
    pub fn symbols(&self, query: &str, limit: usize) -> Vec<WsSymbol> {
        let query = query.trim();
        if query.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for e in self.files.values() {
            let api = &e.api;
            let res = e.resource.map(|id| self.resources[id].name.as_str()).unwrap_or("");
            for d in &api.defs {
                let (name, container) = match (&d.owner, d.path.split_last()) {
                    (Owner::Global(g), None) => (g.as_str(), String::new()),
                    (Owner::Global(g), Some((last, parent))) if parent.len() < 3 => {
                        (last.as_str(), std::iter::once(g.as_str()).chain(parent.iter().map(String::as_str)).collect::<Vec<_>>().join("."))
                    }
                    _ => continue,
                };
                if !fuzzy(name, query) {
                    continue;
                }
                let kind = match (&d.value, d.path.is_empty()) {
                    (Ref::Func(_), _) => WsSymbolKind::Function,
                    (_, true) => WsSymbolKind::Variable,
                    (_, false) => WsSymbolKind::Field,
                };
                out.push(WsSymbol { name: name.to_string(), container, kind, path: e.path.clone(), offset: d.offset });
            }
            let exports = api.exports.iter().map(|x| (&x.name, x.offset)).chain(api.export_calls.iter().map(|x| (&x.name, x.offset)));
            for (n, o) in exports {
                if fuzzy(n, query) {
                    out.push(WsSymbol { name: n.clone(), container: format!("exports.{res}"), kind: WsSymbolKind::Export, path: e.path.clone(), offset: o + 1 });
                }
            }
            for (n, o) in &api.events_handled {
                if fuzzy(n, query) {
                    out.push(WsSymbol { name: n.clone(), container: "event".into(), kind: WsSymbolKind::Event, path: e.path.clone(), offset: o + 1 });
                }
            }
        }
        let q = query.to_lowercase();
        out.sort_by_cached_key(|s| {
            let n = s.name.to_lowercase();
            (n != q, !n.starts_with(&q), !n.contains(&q), s.name.len(), s.name.clone(), s.path.clone(), s.offset)
        });
        out.truncate(limit);
        out
    }

    /// Every event name with the number of handlers and of triggers.
    pub fn event_names(&self) -> Vec<(String, usize, usize)> {
        let mut out: HashMap<&str, (usize, usize)> = HashMap::new();
        for e in self.files.values() {
            for (n, _) in &e.api.events_handled {
                out.entry(n).or_default().0 += 1;
            }
            for (n, _) in &e.api.events_triggered {
                out.entry(n).or_default().1 += 1;
            }
        }
        let mut v: Vec<(String, usize, usize)> = out.into_iter().map(|(n, (h, t))| (n.to_string(), h, t)).collect();
        v.sort();
        v
    }

    /// Where an event is registered (`true`) or triggered.
    pub fn event_sites(&self, name: &str) -> Vec<(PathBuf, u32, bool)> {
        let mut out = Vec::new();
        for e in self.files.values() {
            out.extend(e.api.events_handled.iter().filter(|(n, _)| n == name).map(|(_, o)| (e.path.clone(), *o, true)));
            out.extend(e.api.events_triggered.iter().filter(|(n, _)| n == name).map(|(_, o)| (e.path.clone(), *o, false)));
        }
        out.sort();
        out
    }

    /// Event names the workspace registers or triggers, with where.
    pub fn events(&self) -> HashMap<String, Vec<(PathBuf, u32, bool)>> {
        let mut out: HashMap<String, Vec<(PathBuf, u32, bool)>> = HashMap::new();
        for e in self.files.values() {
            for (n, o) in &e.api.events_handled {
                out.entry(n.clone()).or_default().push((e.path.clone(), *o, true));
            }
            for (n, o) in &e.api.events_triggered {
                out.entry(n.clone()).or_default().push((e.path.clone(), *o, false));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, rel: &str, text: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn local(ws_file: &Path, name: &str) -> Ref {
        let text = std::fs::read_to_string(ws_file).unwrap();
        let p = crate::parse(&text);
        let n = crate::names(&p, &text);
        Ref::Local(n.decls.iter().position(|d| d.name == name).unwrap() as u32)
    }

    fn names(items: &[ApiItem]) -> Vec<&str> {
        items.iter().map(|i| i.name.as_str()).collect()
    }

    /// Synthetic resources in the shapes the frameworks use: a core object
    /// returned by an export, functions exported in a `pairs` loop, a compat
    /// resource that `provide`s another name and registers exports through a
    /// required wrapper, and an ox_lib-like lazy `lib`.
    #[test]
    fn core_object_event_and_unknown_exports() {
        let root = std::env::temp_dir().join(format!("cfx-coreobj-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        write(&root, "qbx/fxmanifest.lua", "server_script 's.lua'\nprovide 'qb-core'\n");
        write(&root, "qbx/s.lua", "local function createQbExport(name, cb)\n  AddEventHandler(('__cfx_export_qb-core_%s'):format(name), function(set) set(cb) end)\nend\ncreateQbExport('GetCoreObject', function() return {} end)\n");
        write(&root, "half/fxmanifest.lua", "server_scripts { 's.lua', 'gone.lua' }\n");
        write(&root, "half/s.lua", "exports('A', function() end)\n");
        write(&root, "a/twice/fxmanifest.lua", "server_script 's.lua'\n");
        write(&root, "a/twice/s.lua", "exports('A', function() end)\n");
        write(&root, "b/twice/fxmanifest.lua", "server_script 's.lua'\n");
        write(&root, "b/twice/s.lua", "exports('B', function() end)\n");
        write(&root, "app/fxmanifest.lua", "server_script 's.lua'\n");
        let text = "TriggerEvent('QBCore:GetObject', function(o) QB = o end)\nexports.half:B()\nexports.twice:C()\n";
        write(&root, "app/s.lua", text);
        let lints = |ws: &Workspace| {
            let path = root.join("app/s.lua");
            let p = crate::parse(text);
            let n = crate::names(&p, text);
            crate::workspace::lint_file(ws, &path, &p, text, &n, &crate::lint::LintConfig::default())
                .into_iter()
                .filter(|l| l.code.starts_with("qb/") || l.code.contains("export"))
                .map(|l| l.code)
                .collect::<Vec<_>>()
        };
        // `half` misses a script, `twice` has two copies: their exports are not known.
        assert_eq!(lints(&Workspace::scan(std::slice::from_ref(&root))), ["qb/core-object-event"]);
        write(&root, "qbx/h.lua", "AddEventHandler('QBCore:GetObject', function(cb) cb({}) end)\n");
        write(&root, "qbx/fxmanifest.lua", "server_scripts { 's.lua', 'h.lua' }\nprovide 'qb-core'\n");
        assert!(lints(&Workspace::scan(std::slice::from_ref(&root))).is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn net_events_only_fired_by_the_server() {
        let root = std::env::temp_dir().join(format!("cfx-netev-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        write(&root, "app/fxmanifest.lua", "server_script 's.lua'\nclient_script 'c.lua'\n");
        let text = "RegisterNetEvent('app:local', function() end)\nRegisterNetEvent('app:fromClient', function() end)\nRegisterNetEvent('app:wrapped', function() end)\nRegisterNetEvent('app:unused', function() end)\nAddEventHandler('app:plain', function() end)\nTriggerEvent('app:local')\nTriggerEvent('app:fromClient')\nTriggerEvent('app:wrapped')\nTriggerEvent('app:plain')\n";
        write(&root, "app/s.lua", text);
        write(&root, "app/c.lua", "TriggerServerEvent('app:fromClient')\nlocal names = { 'app:wrapped' }\n");
        let ws = Workspace::scan(std::slice::from_ref(&root));
        let path = root.join("app/s.lua");
        let p = crate::parse(text);
        let n = crate::names(&p, text);
        let found: Vec<&str> = crate::workspace::lint_file(&ws, &path, &p, text, &n, &crate::lint::LintConfig::default())
            .into_iter()
            .filter(|l| l.code == "fivem/net-event-same-side")
            .map(|l| &text[l.range])
            .collect();
        assert_eq!(found, ["'app:local'"]);
        // The same code on the client is not this rule's case, and a resource
        // with an escrow file may send the event from there.
        write(&root, "app2/fxmanifest.lua", "client_script 'c.lua'\n");
        write(&root, "app2/c.lua", "RegisterNetEvent('app2:local', function() end)\nTriggerEvent('app2:local')\n");
        write(&root, "locked/fxmanifest.lua", "server_scripts { 's.lua', 'x.lua' }\n");
        write(&root, "locked/s.lua", "RegisterNetEvent('locked:local', function() end)\nTriggerEvent('locked:local')\n");
        write(&root, "locked/x.lua", "FXAP\u{1}\u{2}");
        let ws = Workspace::scan(std::slice::from_ref(&root));
        for file in ["app2/c.lua", "locked/s.lua"] {
            let text = std::fs::read_to_string(root.join(file)).unwrap();
            let p = crate::parse(&text);
            let n = crate::names(&p, &text);
            let hits = crate::workspace::lint_file(&ws, &root.join(file), &p, &text, &n, &crate::lint::LintConfig::default())
                .into_iter()
                .filter(|l| l.code == "fivem/net-event-same-side")
                .count();
            assert_eq!(hits, 0, "{file}");
        }
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn esx_shared_object_event() {
        let root = std::env::temp_dir().join(format!("cfx-esxev-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let text = "ESX = nil\nTriggerEvent('esx:getSharedObject', function(obj) ESX = obj end)\nTriggerEvent('other:event')\n";
        write(&root, "app/fxmanifest.lua", "client_script 'c.lua'\n");
        write(&root, "app/c.lua", text);
        let hits = |ws: &Workspace| -> usize {
            let p = crate::parse(text);
            let n = crate::names(&p, text);
            crate::workspace::lint_file(ws, &root.join("app/c.lua"), &p, text, &n, &crate::lint::LintConfig::default())
                .into_iter()
                .filter(|l| l.code == "esx/shared-object-event")
                .count()
        };
        // Without ESX in the workspace nothing is known about the event.
        assert_eq!(hits(&Workspace::scan(std::slice::from_ref(&root))), 0);
        write(&root, "es_extended/fxmanifest.lua", "shared_script 'shared/main.lua'\n");
        write(
            &root,
            "es_extended/shared/main.lua",
            "ESX = {}\n\nexports(\"getSharedObject\", function()\n    return ESX\nend)\n\nAddEventHandler(\"esx:getSharedObject\", function(cb)\n    if ESX.IsFunctionReference(cb) then\n        cb(ESX)\n    end\nend)\n",
        );
        assert_eq!(hits(&Workspace::scan(std::slice::from_ref(&root))), 1);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn undefined_doc_names() {
        let root = std::env::temp_dir().join(format!("cfx-udn-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        write(&root, "app/fxmanifest.lua", "server_scripts { 'types.lua', 's.lua' }\n");
        write(&root, "app/types.lua", "---@class Known\n---@alias Kind 'a'|'b'\n---@enum Colour\nColour = { red = 1 }\n");
        let text = "---@generic T\n---@param a T\n---@param b Known|Kind?\n---@param c Colour[]\n---@param d table<string, Missing>\n---@param e int\n---@param f vector3\n---@param g fun(x: Other): boolean\n---@return Known, string\nlocal function f(a, b, c, d, e, f, g) end\n";
        write(&root, "app/s.lua", text);
        // A resource with an escrow file may declare anything there.
        write(&root, "locked/fxmanifest.lua", "server_script 's.lua'\n");
        write(&root, "locked/s.lua", text);
        write(&root, "locked/x.lua", "FXAP\u{1}\u{2}");
        std::fs::write(root.join("locked/.fxap"), "x").unwrap();
        let ws = Workspace::scan(std::slice::from_ref(&root));
        let p = crate::parse(text);
        let n = crate::names(&p, text);
        let found = |res: &str| -> Vec<String> {
            crate::workspace::lint_file(&ws, &root.join(res).join("s.lua"), &p, text, &n, &crate::lint::LintConfig::default())
                .into_iter()
                .filter(|l| l.code == "undefined-doc-name")
                .map(|l| format!("{} {}", &text[l.range], l.message))
                .collect()
        };
        assert_eq!(
            found("app"),
            [
                "Missing Undefined type or alias `Missing`.",
                "int Undefined type or alias `int` (Lua's is `integer`).",
                "Other Undefined type or alias `Other`."
            ]
        );
        assert_eq!(found("locked"), Vec::<String>::new());
        // A resource depending on one with escrow files.
        write(&root, "user/fxmanifest.lua", "dependency 'locked'
server_script 's.lua'
");
        write(&root, "user/s.lua", text);
        write(&root, "plain/fxmanifest.lua", "dependency 'app'
server_script 's.lua'
");
        write(&root, "plain/s.lua", text);
        let ws = Workspace::scan(std::slice::from_ref(&root));
        let found = |res: &str| -> usize {
            crate::workspace::lint_file(&ws, &root.join(res).join("s.lua"), &p, text, &n, &crate::lint::LintConfig::default())
                .into_iter()
                .filter(|l| l.code == "undefined-doc-name")
                .count()
        };
        assert_eq!((found("user"), found("plain")), (0, 3));
        // A class declared in the unsaved text itself, not yet in the index.
        let fresh = "---@class Fresh
---@alias FreshKind 'x'
---@type Fresh|FreshKind
local v
";
        let p = crate::parse(fresh);
        let n = crate::names(&p, fresh);
        let hits = crate::workspace::lint_file(&ws, &root.join("app/s.lua"), &p, fresh, &n, &crate::lint::LintConfig::default())
            .into_iter()
            .filter(|l| l.code == "undefined-doc-name")
            .count();
        assert_eq!(hits, 0);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn inlay_parameter_names_line_up() {
        let root = std::env::temp_dir().join(format!("cfx-inlay-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        write(&root, "app/fxmanifest.lua", "client_scripts { 'm.lua', 'use.lua' }
");
        write(&root, "app/m.lua", "M = {}
function M.f(self, a) end
function M:g(b) end
exports('Ex', function(x) end)
");
        let text = "M:f(1)
M.g(M, 2)
M:g(3)
exports.app:Ex(5)
local L = {}
function L.f(self, c) end
L:f(6)
local s = 'x'
local r = s:rep(7)
";
        write(&root, "app/use.lua", text);
        let ws = Workspace::scan(std::slice::from_ref(&root));
        let path = root.join("app/use.lua");
        let p = crate::parse(text);
        let n = crate::names(&p, text);
        let cx = crate::FileCtx { game: crate::library::Game::Gta5, side: FileSide::Client, ws: Some((&ws, path.as_path())) };
        let hints: Vec<String> = crate::inlay_hints(&p, &n, text, crate::TextRange::up_to(crate::TextSize::of(text)), cx)
            .into_iter()
            .map(|h| format!("{}{}", h.label, &text[usize::from(h.offset)..usize::from(h.offset) + 1]))
            .collect();
        let names: Vec<&String> = hints.iter().filter(|h| !h.starts_with(": ")).collect();
        assert_eq!(names, ["a:1", "self:M", "b:2", "b:3", "x:5", "c:6"], "{hints:?}");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn doc_types_and_classes() {
        let root = std::env::temp_dir().join(format!("cfx-types-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // ESX 1.12: `---@return table`, the player stored under a computed key,
        // the class on a local inside the constructor.
        write(&root, "esx12/fxmanifest.lua", "server_scripts { 'player.lua', 'functions.lua', 'main.lua', 'use.lua' }\nclient_script 'client.lua'\n");
        write(
            &root,
            "esx12/player.lua",
            "---@class xPlayer\n---@field identifier string\n---@field job table\n\nfunction CreateExtendedPlayer(playerId, identifier)\n    ---@class xPlayer\n    local self = {}\n    self.source = playerId\n    function self.getMoney()\n        return 0\n    end\n    return self\nend\n",
        );
        write(
            &root,
            "esx12/functions.lua",
            "ESX = {}\nESX.Players = {}\n---@param source number\n---@return table\nfunction ESX.GetPlayerFromId(source)\n    return ESX.Players[tonumber(source)]\nend\n",
        );
        let main = "local function load(playerId)\n    local xPlayer = CreateExtendedPlayer(playerId, 'id')\n    ESX.Players[playerId] = xPlayer\nend\n";
        write(&root, "esx12/main.lua", main);
        write(&root, "esx12/use.lua", "");
        let client = "local ped = PlayerPedId()\n";
        write(&root, "esx12/client.lua", client);
        // ESX 1.13: classes with `fun` fields and a parent.
        write(&root, "esx13/fxmanifest.lua", "server_scripts { 'player.lua', 'functions.lua' }\n");
        write(
            &root,
            "esx13/player.lua",
            "---@class StaticPlayer\n---@field getAccounts fun(minimal?: boolean): table\n---@field job ESXJob\n\n---@class xPlayer: StaticPlayer\n---@field name string\n\n---@class ESXJob\n---@field grade integer\n",
        );
        let f13 = "ESX13 = {}\n---@param id number\n---@return xPlayer?\nfunction ESX13.Get(id) end\n";
        write(&root, "esx13/functions.lua", f13);
        let ws = Workspace::scan(std::slice::from_ref(&root));
        let names = |file: &str, text: &str, expr: &str| -> Vec<String> {
            let mut v: Vec<String> =
                ws.members_of_expression(&root.join(file), text, expr).unwrap_or_default().into_iter().map(|i| i.name).collect();
            v.sort();
            v
        };
        assert_eq!(names("esx12/main.lua", main, "ESX.GetPlayerFromId(1)"), ["getMoney", "identifier", "job", "source"]);
        assert_eq!(names("esx13/functions.lua", f13, "ESX13.Get(1)"), ["getAccounts", "job", "name"]);
        assert_eq!(names("esx13/functions.lua", f13, "ESX13.Get(1).job"), ["grade"]);
        let items = ws.members_of_expression(&root.join("esx13/functions.lua"), f13, "ESX13.Get(1)").unwrap();
        let acc = items.iter().find(|i| i.name == "getAccounts").unwrap();
        assert_eq!(acc.params.as_deref(), Some(&["minimal".to_string()][..]));
        assert_eq!(acc.ty.as_deref(), Some("fun(minimal?: boolean):table"));
        let get = ws.api_item(&root.join("esx13/functions.lua"), None, &Ref::Global("ESX13".into()), &["Get".into()]).unwrap();
        assert_eq!(get.ty.as_deref(), Some("fun(id: number):xPlayer?"));
        // A native's `---@return vector3`.
        let coords = names("esx12/client.lua", client, "GetEntityCoords(ped)");
        assert!(["x", "y", "z", "xy", "xyz"].iter().all(|n| coords.iter().any(|c| c == n)), "{coords:?}");

        // The editor, on unsaved text (`$`: cursor), inside functions.
        let at = |file: &str, side: FileSide, t: &str| {
            let o = crate::TextSize::from(t.find('$').unwrap() as u32);
            let text = t.replacen('$', "", 1);
            (root.join(file), side, text, o)
        };
        let labels = |file: &str, side: FileSide, t: &str| -> Vec<String> {
            let (path, side, text, o) = at(file, side, t);
            let cx = crate::FileCtx { game: crate::library::Game::Gta5, side, ws: Some((&ws, &path)) };
            let p = crate::parse(&text);
            let n = crate::names(&p, &text);
            let mut v: Vec<String> = crate::complete(&p, &n, &text, o, cx).map(|c| c.items).unwrap_or_default().into_iter().map(|i| i.label).collect();
            v.sort();
            v
        };
        let hover = |file: &str, side: FileSide, t: &str| -> String {
            let (path, side, text, o) = at(file, side, t);
            let cx = crate::FileCtx { game: crate::library::Game::Gta5, side, ws: Some((&ws, &path)) };
            let p = crate::parse(&text);
            let n = crate::names(&p, &text);
            crate::hover(&p, &n, &text, o, cx).map(|h| h.markdown).unwrap_or_default()
        };
        let body = "local function f(src)\n    local xPlayer = ESX.GetPlayerFromId(src)\n    xPlayer.$\nend\n";
        let l = labels("esx12/use.lua", FileSide::Server, body);
        assert!(["getMoney", "identifier", "source"].iter().all(|n| l.iter().any(|x| x == n)), "{l:?}");
        let l = labels("esx12/use.lua", FileSide::Server, &body.replace("xPlayer.$", "xPlayer:$"));
        assert_eq!(l, ["getMoney"]);
        let h = hover("esx12/use.lua", FileSide::Server, "local function f(src)\n    local x$Player = ESX.GetPlayerFromId(src)\nend\n");
        assert!(h.contains("local xPlayer: xPlayer"), "{h}");
        let l = labels("esx12/client.lua", FileSide::Client, "local c = GetEntityCoords(PlayerPedId())\nprint(GetEntityCoords(PlayerPedId()).$)\n");
        assert!(["x", "y", "z"].iter().all(|n| l.iter().any(|x| x == n)), "{l:?}");
        let l = labels("esx12/client.lua", FileSide::Client, "local c = GetEntityCoords(PlayerPedId())\nprint(c.$)\n");
        assert!(["x", "y", "z"].iter().all(|n| l.iter().any(|x| x == n)), "{l:?}");
        let typed = "---@param p xPlayer\nlocal function g(p)\n    p.$\nend\n";
        assert_eq!(labels("esx13/functions.lua", FileSide::Server, typed), ["getAccounts", "job", "name"]);
        let h = hover("esx13/functions.lua", FileSide::Server, "---@param p xPlayer\nlocal function g($p)\nend\n");
        assert!(h.contains("(parameter) p: xPlayer"), "{h}");
        let h = hover("esx13/functions.lua", FileSide::Server, "---@param p xPlayer\nlocal function g(p)\n    p.getAcc$ounts()\nend\n");
        assert!(h.contains("function getAccounts(minimal?: boolean):table"), "{h}");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn manifest_against_the_disk() {
        let root = std::env::temp_dir().join(format!("cfx-manifest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        write(&root, "lib/fxmanifest.lua", "shared_script 'init.lua'\n");
        write(&root, "lib/init.lua", "X = 1\n");
        let manifest = "client_scripts { 'client/main.lua', 'client/Gone.lua', 'client/*.lua' }\nserver_script 'Server/main.lua'\nshared_scripts { '@lib/init.lua', '@lib/missing.lua', '@elsewhere/a.lua' }\nfiles { 'html/*.png', 'html/index.html', 'node_modules/pkg/index.js', 'node_modules/**/*.js', 'node_modules/gone.js' }\nui_page 'https://example.com/'\n";
        write(&root, "app/fxmanifest.lua", manifest);
        write(&root, "app/client/main.lua", "print(X)\n");
        write(&root, "app/server/main.lua", "print(X)\n");
        write(&root, "app/html/index.html", "<p></p>\n");
        write(&root, "app/node_modules/pkg/index.js", "\n");
        let ws = Workspace::scan(std::slice::from_ref(&root));
        let path = root.join("app/fxmanifest.lua");
        let p = crate::parse(manifest);
        let n = crate::names(&p, manifest);
        let found: Vec<(&str, &str)> = crate::workspace::lint_file(&ws, &path, &p, manifest, &n, &crate::lint::LintConfig::default())
            .into_iter()
            .map(|l| (l.code, &manifest[l.range]))
            .collect();
        assert_eq!(
            found,
            [
                ("fivem/manifest-missing-file", "'client/Gone.lua'"),
                ("fivem/manifest-path-case", "'Server/main.lua'"),
                ("fivem/manifest-missing-import", "'@lib/missing.lua'"),
                ("fivem/manifest-no-match", "'html/*.png'"),
                ("fivem/manifest-missing-file", "'node_modules/gone.js'"),
            ]
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn frameworks_from_the_workspace() {
        let root = std::env::temp_dir().join(format!("cfx-api-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        write(&root, "mycore/fxmanifest.lua", "server_scripts { 'server/main.lua', 'server/functions.lua' }\nclient_script 'client/main.lua'\n");
        write(&root, "mycore/server/main.lua", "Core = {}\nCore.Functions = {}\nCore.Players = {}\nexports('GetCoreObject', function() return Core end)\n");
        write(
            &root,
            "mycore/server/functions.lua",
            "---Gets a player.\nfunction Core.Functions.GetPlayer(source) return Core.Players[source] end\n---@deprecated use GetPlayer\nfunction Core.Functions.OldGet(id) end\nfor name, fn in pairs(Core.Functions) do exports(name, fn) end\n",
        );
        write(&root, "mycore/client/main.lua", "exports('ClientOnly', function(a) end)\n");
        write(
            &root,
            "newcore/fxmanifest.lua",
            "server_script 'bridge/main.lua'\nfiles { 'bridge/export.lua', 'bridge/functions.lua' }\nprovide 'oldcore'\n",
        );
        write(
            &root,
            "newcore/bridge/export.lua",
            "return function(name, cb)\n  AddEventHandler(('__cfx_export_oldcore_%s'):format(name), function(set) set(cb) end)\nend\n",
        );
        write(&root, "newcore/bridge/functions.lua", "local functions = {}\n---@deprecated use lib.notify\nfunction functions.Notify(text) end\nreturn functions\n");
        write(
            &root,
            "newcore/bridge/main.lua",
            "local compat = {}\ncompat.Functions = require 'bridge.functions'\nlocal createExport = require 'bridge.export'\ncreateExport('GetCoreObject', function() return compat end)\n",
        );
        write(&root, "oxl/fxmanifest.lua", "shared_script 'resource/init.lua'\nfiles { 'init.lua', 'imports/**' }\n");
        write(&root, "oxl/init.lua", "lib = setmetatable({ name = 'oxl' }, {})\n");
        write(&root, "oxl/imports/notify/client.lua", "---Shows a notification.\nfunction lib.notify(data) end\nreturn lib.notify\n");
        write(&root, "oxl/imports/callback/server.lua", "lib.callback = {}\nfunction lib.callback.register(name, cb) end\n");
        write(&root, "app/fxmanifest.lua", "shared_script '@oxl/init.lua'\nclient_script 'client.lua'\nserver_script 'server.lua'\n");
        write(&root, "app/client.lua", "lib.notify({})\n");
        write(&root, "app/server.lua", "local C = exports.mycore:GetCoreObject()\nlocal Q = exports['oldcore']:GetCoreObject()\n");
        let ws = Workspace::scan(std::slice::from_ref(&root));
        let server = root.join("app/server.lua");
        let client = root.join("app/client.lua");

        let f = ws.api_members(&server, None, &local(&server, "C"), &["Functions".into()]);
        assert_eq!(names(&f), ["GetPlayer", "OldGet"]);
        assert_eq!(f[0].params.as_deref(), Some(&["source".to_string()][..]));
        assert_eq!(f[0].doc, "Gets a player.");
        assert_eq!(f[1].deprecated(), Some("use GetPlayer"));
        let core = ws.api_members(&server, None, &local(&server, "C"), &[]);
        assert_eq!(names(&core), ["Functions", "Players"]);

        let e = ws.exports_on("mycore", FileSide::Server).unwrap();
        assert_eq!(names(&e.items), ["GetCoreObject", "GetPlayer", "OldGet"]);
        assert_eq!(names(&e.other_side), ["ClientOnly"]);
        assert!(e.complete);

        let e = ws.exports_on("oldcore", FileSide::Server).unwrap();
        assert_eq!(e.resource, "newcore");
        assert_eq!(names(&e.items), ["GetCoreObject"]);
        let q = ws.api_members(&server, None, &local(&server, "Q"), &["Functions".into()]);
        assert_eq!(names(&q), ["Notify"]);
        assert_eq!(q[0].deprecated(), Some("use lib.notify"));

        let lib = Ref::Global("lib".into());
        let c = ws.api_members(&client, None, &lib, &[]);
        assert!(names(&c).contains(&"notify") && !names(&c).contains(&"callback"), "{:?}", names(&c));
        let n = c.iter().find(|i| i.name == "notify").unwrap();
        assert_eq!((n.params.as_deref(), n.doc.as_str()), (Some(&["data".to_string()][..]), "Shows a notification."));
        let s = ws.api_members(&server, None, &lib, &["callback".into()]);
        assert_eq!(names(&s), ["register"]);
        let item = ws.api_item(&server, None, &local(&server, "C"), &["Functions".into(), "GetPlayer".into()]).unwrap();
        assert!(item.location.unwrap().0.ends_with("functions.lua"));

        let s = ws.symbols("getpl", 10);
        assert_eq!((s[0].name.as_str(), s[0].container.as_str(), s[0].kind), ("GetPlayer", "Core.Functions", WsSymbolKind::Function), "{s:?}");
        assert!(ws.symbols("GetCoreObject", 10).iter().any(|s| s.kind == WsSymbolKind::Export && s.container == "exports.mycore"));
        assert!(ws.symbols("", 10).is_empty());
        let r = ws.export_resources();
        assert!(["mycore", "newcore", "oldcore"].iter().all(|n| r.contains(&n.to_string())), "{r:?}");
        assert!(ws.has_deprecated("OldGet") && ws.has_deprecated("Notify"));

        // The editor features over the same workspace, on unsaved text (`$`: cursor).
        let cx = crate::FileCtx { game: crate::library::Game::Gta5, side: FileSide::Server, ws: Some((&ws, &server)) };
        let doc = |t: &str| {
            let o = t.find('$').unwrap();
            let text = format!("local C = exports.mycore:GetCoreObject()\n{}", t.replacen('$', "", 1));
            let o = crate::TextSize::from((o + text.len() - t.len() + 1) as u32);
            (text, o)
        };
        let complete = |t: &str| {
            let (text, o) = doc(t);
            let p = crate::parse(&text);
            let n = crate::names(&p, &text);
            crate::complete(&p, &n, &text, o, cx).unwrap().items
        };
        let labels = |t: &str| complete(t).into_iter().map(|i| i.label).collect::<Vec<_>>();
        assert_eq!(labels("C.Functions.$"), ["GetPlayer", "OldGet"]);
        assert!(complete("C.Functions.$").iter().find(|i| i.label == "OldGet").unwrap().deprecated);
        let l = labels("exports.mycore:$");
        assert!(l.contains(&"GetPlayer".to_string()) && !l.contains(&"ClientOnly".to_string()), "{l:?}");
        assert_eq!(labels("exports['oldcore']:$"), ["GetCoreObject"]);
        let (text, o) = doc("C.Functions.GetPlayer(1, $");
        let (p, n) = (crate::parse(&text), crate::names(&crate::parse(&text), &text));
        let sig = crate::signature_help(&p, &n, &text, o, cx).unwrap();
        assert_eq!((sig.signatures[0].label.as_str(), sig.active_param), ("GetPlayer(source)", 1));
        let (text, o) = doc("C.Functions.GetPl$ayer(1)\nexports.mycore:GetCoreObject()");
        let p = crate::parse(&text);
        let n = crate::names(&p, &text);
        let h = crate::hover(&p, &n, &text, o, cx).unwrap();
        assert!(h.markdown.contains("function GetPlayer(source)") && h.markdown.contains("Gets a player.") && h.markdown.contains("mycore/server/functions.lua:2"), "{}", h.markdown);
        let (file, _) = crate::api_definition(&p, &n, &text, o, &ws, &server).unwrap();
        assert!(file.ends_with("functions.lua"));
        let o = crate::TextSize::from(text.rfind("GetCoreObject").unwrap() as u32);
        let h = crate::hover(&p, &n, &text, o, cx).unwrap();
        assert!(h.markdown.contains("exports['mycore']:GetCoreObject()"), "{}", h.markdown);

        // The workspace rules.
        let text = "local C = exports.mycore:GetCoreObject()\nC.Functions.OldGet(1)\nexports.mycore:GetPlayr(1)\nexports.mycore:ClientOnly()\nexports.oldcore:GetCoreObject()\nexports.newcore:GetCoreObject()\nexports.nowhere:Anything()\n";
        let p = crate::parse(text);
        let n = crate::names(&p, text);
        let found: Vec<(&str, &str, String)> = crate::workspace::lint_file(&ws, &server, &p, text, &n, &crate::lint::LintConfig::default())
            .into_iter()
            .filter(|l| l.code == "deprecated" || l.code.contains("export"))
            .map(|l| (l.code, &text[l.range], l.message))
            .collect();
        let codes: Vec<(&str, &str)> = found.iter().map(|(c, t, _)| (*c, *t)).collect();
        // `newcore` registers its exports under `oldcore` only.
        assert_eq!(
            codes,
            [
                ("deprecated", "C.Functions.OldGet"),
                ("fivem/unknown-export", "GetPlayr"),
                ("fivem/export-wrong-side", "ClientOnly"),
                ("fivem/unknown-export", "GetCoreObject"),
            ],
            "{found:?}"
        );
        assert!(found[0].2.contains("use GetPlayer") && found[1].2.contains("Did you mean `GetPlayer`?"), "{found:?}");
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// The arity rules beyond LuaLS: a parameter the body tests for nil is not
    /// required; a shadowed library function and a local assigned again are
    /// left alone.
    #[test]
    fn arity_beyond_luals() {
        let root = std::env::temp_dir().join(format!("cfx-arity-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        write(&root, "lib/fxmanifest.lua", "shared_script 'l.lua'\n");
        write(&root, "lib/l.lua", "Lib = {}\nfunction Lib.one(a) end\n");
        write(&root, "app/fxmanifest.lua", "shared_scripts { '@lib/l.lua', 'a.lua', 's.lua' }\n");
        write(&root, "app/s.lua", "print = function() end\n");
        let text = "\
---@param key string
---@param subs table
local function t(key, subs)
    if not subs then return key end
    return key
end
t('a')
---@param a integer
---@param b integer
local function both(a, b) return a + b end
both(1)
both(1, 2, 3)
Lib.one(1, 2)
print(1, 2)
local ok, f = pcall(require, 'x')
if not ok then f = function() end end
f(1)
";
        write(&root, "app/a.lua", text);
        let ws = Workspace::scan(std::slice::from_ref(&root));
        let file = root.join("app/a.lua");
        let p = crate::parse(text);
        let n = crate::names(&p, text);
        let lints = crate::workspace::lint_file(&ws, &file, &p, text, &n, &crate::lint::LintConfig::default());
        let got: Vec<(&str, &str)> = lints
            .iter()
            .filter(|l| l.code.ends_with("-parameter"))
            .map(|l| (l.code, &text[l.range]))
            .collect();
        // `@lib/l.lua` in the manifest makes `Lib.one` part of the resource.
        assert_eq!(got, [("missing-parameter", "both(1)"), ("redundant-parameter", "3"), ("redundant-parameter", "2")]);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn undefined_field_own_classes_only() {
        let root = std::env::temp_dir().join(format!("cfx-undef-field-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        write(&root, "app/fxmanifest.lua", "shared_scripts { 'a.lua', 'b.lua', 'c.lua' }\n");
        // Another file writes `Lib.state`; another declares `Shared` too.
        write(&root, "app/b.lua", "Lib.state = 1\n");
        write(&root, "app/c.lua", "---@class Shared\n---@field extra integer\n");
        let text = "---@class Lib\n---@field name string\nLib = {}\nfunction Lib:go() end\n\n---@class Shared\nlocal S = {}\n\n---@type Lib\nlocal l\n---@type Shared\nlocal s\nprint(l.name, l.state, l.missing, s.extra, s.other, S)\nl:go()\nl:gone()\n";
        write(&root, "app/a.lua", text);
        let ws = Workspace::scan(std::slice::from_ref(&root));
        let file = root.join("app/a.lua");
        let p = crate::parse(text);
        let n = crate::names(&p, text);
        let lints = crate::workspace::lint_file(&ws, &file, &p, text, &n, &crate::lint::LintConfig::default());
        let got: Vec<&str> = lints.iter().filter(|l| l.code == "undefined-field").map(|l| &text[l.range]).collect();
        assert_eq!(got, ["missing", "gone"]);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn private_and_protected_names() {
        let root = std::env::temp_dir().join(format!("cfx-docnames-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        write(&root, "app/fxmanifest.lua", "shared_script 'a.lua'\n");
        let text = "---@class A\n---@field _id_ number\n---@field _user number\n\n---@type A\nlocal t\nprint(t._id_, t._user)\n---@class B: A\nlocal t2\nprint(t2._id_, t2._user)\n";
        write(&root, "app/a.lua", text);
        let ws = Workspace::scan(std::slice::from_ref(&root));
        let file = root.join("app/a.lua");
        let p = crate::parse(text);
        let n = crate::names(&p, text);
        let mut config = crate::lint::LintConfig::default();
        let invisible = |config: &crate::lint::LintConfig| -> Vec<&str> {
            crate::workspace::lint_file(&ws, &file, &p, text, &n, config).iter().filter(|l| l.code == "invisible").map(|l| &text[l.range]).collect()
        };
        assert!(invisible(&config).is_empty());
        config.private_names = vec!["_*_".into()];
        config.protected_names = vec!["_*".into()];
        // `t` is outside A: both hidden; in B (a subclass) only the private one.
        assert_eq!(invisible(&config), ["_id_", "_user", "_id_"]);
        // `Lua.doc.regengine` `lua`: Lua patterns, found anywhere in the name.
        // The globs read as Lua patterns: `_*_` finds a `_` in any name.
        config.lua_name_patterns = true;
        assert_eq!(invisible(&config), ["_id_", "_user", "_id_", "_user"]);
        // LuaLS's own case (test/diagnostics/invisible.lua).
        config.private_names = vec!["^_[%w_]*%w$".into()];
        config.protected_names = vec!["^_[%w_]*_$".into()];
        assert_eq!(invisible(&config), ["_id_", "_user", "_user"]);
        config.private_names = vec!["d_".into()];
        config.protected_names = vec![];
        assert_eq!(invisible(&config), ["_id_", "_id_"]);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn private_names_on_table_fields() {
        let root = std::env::temp_dir().join(format!("cfx-docnames2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        write(&root, "app/fxmanifest.lua", "shared_script 'a.lua'\n");
        // A constructor field, a field written on the class, one written in a
        // method; `_Up` does not match `_u*` (case-sensitive, as LuaLS).
        let text = "---@class A\nlocal A = {\n    _id = 0,\n    _Up = 1,\n}\nA._count = 0\nfunction A:init() self._name = 'a' end\n\n---@type A\nlocal t\nprint(t._id, t._Up, t._count, t._name)\n";
        write(&root, "app/a.lua", text);
        let ws = Workspace::scan(std::slice::from_ref(&root));
        let file = root.join("app/a.lua");
        let p = crate::parse(text);
        let n = crate::names(&p, text);
        let config = crate::lint::LintConfig { private_names: vec!["_i*".into(), "_c*".into(), "_n*".into(), "_u*".into()], ..Default::default() };
        let got: Vec<&str> =
            crate::workspace::lint_file(&ws, &file, &p, text, &n, &config).iter().filter(|l| l.code == "invisible").map(|l| &text[l.range]).collect();
        assert_eq!(got, ["_id", "_count", "_name"]);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn classes_from_dependencies_first() {
        // Two resources declare `Config`; `app` depends on `b`: `b`'s wins.
        let root = std::env::temp_dir().join(format!("cfx-scoped-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        write(&root, "a/fxmanifest.lua", "shared_script 'c.lua'\n");
        write(&root, "a/c.lua", "---@class Config\n---@field size string\nConfig = {}\n");
        write(&root, "b/fxmanifest.lua", "shared_script 'c.lua'\n");
        write(&root, "b/c.lua", "---@class Config\n---@field size integer\nConfig = {}\n");
        write(&root, "app/fxmanifest.lua", "dependency 'b'\nshared_script 'main.lua'\n");
        let text = "---@type Config\nlocal cfg\nlocal v = cfg.size\nprint(v)\n";
        write(&root, "app/main.lua", text);
        let ws = Workspace::scan(std::slice::from_ref(&root));
        let file = root.join("app/main.lua");
        let p = crate::parse(text);
        let n = crate::names(&p, text);
        let v = n.decls.iter().position(|d| d.name == "v").unwrap();
        let overlay = crate::api::file_api_full(&p, text, &n);
        assert_eq!(ws.type_views(&file, Some(&overlay), &Ref::Local(v as u32)), ["integer"]);
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// different-requires across the files of a resource; inject-field only on
    /// classes no other file declares.
    #[test]
    fn requires_and_injected_fields() {
        let root = std::env::temp_dir().join(format!("cfx-requires-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        write(&root, "app/fxmanifest.lua", "shared_scripts { 'a.lua', 'b.lua' }\nfiles { 'mod/*.lua' }\n");
        write(&root, "app/mod/util.lua", "return {}\n");
        write(&root, "app/b.lua", "local U = require 'mod.util'\nreturn U\n");
        let text = "local A = require 'mod/util'\nlocal B = require 'mod.util'\n---@class Box\n---@field size integer\nlocal Box = {}\nBox.kind = 1\n---@type Box\nlocal b\nb.size = 1\nb.kind = 2\nb.color = 3\nprint(A, B, b)\n";
        write(&root, "app/a.lua", text);
        let ws = Workspace::scan(std::slice::from_ref(&root));
        let file = root.join("app/a.lua");
        let p = crate::parse(text);
        let n = crate::names(&p, text);
        let lints = crate::workspace::lint_file(&ws, &file, &p, text, &n, &crate::lint::LintConfig::default());
        let got: Vec<(&str, &str)> = lints.iter().filter(|l| matches!(l.code, "different-requires" | "inject-field")).map(|l| (l.code, &text[l.range])).collect();
        assert_eq!(got, [("different-requires", "'mod/util'"), ("inject-field", "color")]);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
