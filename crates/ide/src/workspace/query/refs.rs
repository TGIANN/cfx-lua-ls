//! References and rename by type, across the workspace (LuaLS
//! `core/reference.lua`, `core/rename.lua`): class, alias and enum names in
//! doc comments, and class fields by the class that declares them: its
//! `---@field`, `C.x = v`, `function C:x()`, constructor keys of a value of
//! the class, and `o.x` / `o:x()` / `o["x"]` wherever `o` is a value of the
//! class or of a subclass.
//!
//! A name counts in a file only when that file sees the same declaring files
//! (the resource tiers of `class_files`), so two resources' own `Config`
//! classes stay apart.

use super::{Anchor, Base, Cx, Q, Val, builtin_type};
use crate::api::{self, Owner, Ref};
use crate::luacats::{Key, Tag};
use crate::manifest::FileSide;
use crate::workspace::{Workspace, key};
use crate::{Names, Parse, TextRange, TextSize};
use cfx_lua_syntax::{SyntaxKind::*, SyntaxNode};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// What a type-aware reference or rename is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeTarget {
    /// A `---@class` / `---@alias` / `---@enum` name.
    DocName(String),
    /// Field `name` of the class `owner`, the one in the chain declaring it.
    Field { owner: String, name: String },
}

/// The ranges per file; the cursor's file first.
pub type FileRanges = Vec<(PathBuf, Vec<TextRange>)>;

/// One file read for a search.
struct Scan {
    key: String,
    path: PathBuf,
    text: String,
    parse: Parse,
    names: Names,
    api: api::FileApi,
}

impl Scan {
    fn new(path: &Path, text: String) -> Scan {
        let parse = crate::parse(&text);
        let names = crate::names(&parse, &text);
        let api = api::file_api_full(&parse, &text, &names);
        Scan { key: key(path), path: path.to_path_buf(), text, parse, names, api }
    }
}

/// A field site: the key's range and the expression or table it belongs to.
enum Holder {
    /// `o.x`, `o:x()`, `o["x"]`: the object expression.
    Expr(SyntaxNode),
    /// `function a.b:x()`: the names before the key.
    FuncName(SyntaxNode),
    /// A constructor key: the table.
    Table(SyntaxNode),
    /// `---@field x` under `---@class C`.
    Class(String),
}

/// Every place in the file where a field named `name` is read, written or
/// declared; the range is the key (a string key's content).
fn field_sites(scan: &Scan, name: &str) -> Vec<(TextRange, Holder)> {
    let mut out = Vec::new();
    let root = scan.parse.syntax();
    for n in root.descendants() {
        match n.kind() {
            INDEX_EXPR => {
                let Some(object) = n.first_child() else { continue };
                if let Some(f) = crate::lint::field_name(&n) {
                    if f.text() == name {
                        out.push((f.text_range(), Holder::Expr(object)));
                    }
                } else if let Some(k) = n.children().nth(1)
                    && let Some((content, _, range)) = crate::fields::string_key(&k)
                    && content == name
                {
                    out.push((range, Holder::Expr(object)));
                }
            }
            METHOD_CALL_EXPR => {
                if let (Some(object), Some(m)) = (n.first_child(), api::method_name(&n))
                    && m.text() == name
                {
                    out.push((m.text_range(), Holder::Expr(object)));
                }
            }
            FUNC_NAME => {
                let parts: Vec<_> = crate::util::tokens(&n).filter(|t| t.kind() == NAME).collect();
                if parts.len() >= 2 && parts.last().is_some_and(|t| t.text() == name) {
                    out.push((parts[parts.len() - 1].text_range(), Holder::FuncName(n.clone())));
                }
            }
            FIELD_NAMED => {
                let Some(k) = crate::util::tokens(&n).find(|t| t.kind() == NAME) else { continue };
                if k.text() == name
                    && let Some(table) = n.parent()
                {
                    out.push((k.text_range(), Holder::Table(table)));
                }
            }
            FIELD_INDEXED => {
                if let Some(k) = n.first_child()
                    && let Some((content, _, range)) = crate::fields::string_key(&k)
                    && content == name
                    && let Some(table) = n.parent()
                {
                    out.push((range, Holder::Table(table)));
                }
            }
            _ => {}
        }
    }
    for g in crate::lint::code_doc_groups(&root, &scan.text) {
        let mut class = None;
        for t in &g.tags {
            match &t.tag {
                Tag::Class { name: c, .. } => class = Some(c.clone()),
                Tag::Field { key: Key::Name(f), .. } if f == name => {
                    if let (Some(c), Some(r)) = (&class, t.name) {
                        out.push((r, Holder::Class(c.clone())));
                    }
                }
                _ => {}
            }
        }
    }
    out
}

/// The class, alias and enum names written in the file's doc comments, as
/// (range, declaration); generics of a group shadow a class of their name.
fn doc_name_sites(scan: &Scan, name: &str) -> Vec<(TextRange, bool)> {
    let mut out = Vec::new();
    let root = scan.parse.syntax();
    let text = &scan.text;
    for g in crate::lint::code_doc_groups(&root, text) {
        let generic = g.tags.iter().any(|t| match &t.tag {
            Tag::Generic { names } => names.iter().any(|(n, _)| n == name),
            Tag::Class { generics, .. } | Tag::Alias { generics, .. } => generics.iter().any(|n| n == name),
            _ => false,
        });
        if generic {
            continue;
        }
        for t in &g.tags {
            if matches!(&t.tag, Tag::Class { name: n, .. } | Tag::Alias { name: n, .. } | Tag::Enum { name: n, .. } if n == name)
                && let Some(r) = t.name
            {
                out.push((r, true));
            }
            out.extend(
                t.types
                    .iter()
                    .filter(|r| text.get(std::ops::Range::<usize>::from(**r)) == Some(name))
                    .map(|r| (*r, false)),
            );
        }
    }
    for tok in root
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| matches!(t.kind(), COMMENT | BLOCK_COMMENT) && t.text().starts_with("--[["))
    {
        for r in crate::luacats::inline_as_types(tok.text(), tok.text_range().start()) {
            if text.get(std::ops::Range::<usize>::from(r)) == Some(name) {
                out.push((r, false));
            }
        }
    }
    out.sort_by_key(|(r, _)| r.start());
    out.dedup();
    out
}

impl<'a> Q<'a> {
    /// The files declaring a class, alias or enum name, by the resource tiers
    /// of `class_files`, sorted.
    fn doc_name_files(&self, name: &str, res: Option<usize>) -> Vec<&'a str> {
        let mut out = self.class_files(name, res);
        if out.is_empty() {
            let files: Vec<&'a str> = self
                .ws
                .files
                .iter()
                .filter(|(_, e)| e.api.doc_names.iter().any(|n| n == name))
                .map(|(k, _)| k.as_str())
                .collect();
            let of = |set: &[usize]| -> Vec<&'a str> {
                files
                    .iter()
                    .copied()
                    .filter(|f| self.entry(f).and_then(|e| e.resource).is_some_and(|r| set.contains(&r)))
                    .collect()
            };
            let res = res.filter(|_| files.len() > 1);
            let own = res.map(|r| of(&[r])).unwrap_or_default();
            let related =
                if own.is_empty() { res.map(|r| of(&self.ws.related(r))).unwrap_or_default() } else { Vec::new() };
            out = if !own.is_empty() {
                own
            } else if !related.is_empty() {
                related
            } else {
                files
            };
            if let Some((k, a)) = self.overlay
                && !out.contains(&k)
                && a.doc_names.iter().any(|n| n == name)
            {
                out.push(k);
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The classes a value is: `---@type C`, `---@class C` on its table, a
    /// `---@return C` it comes from, a `---@field f C` it is read through.
    fn ref_classes(&self, r: &Ref, cx: &Cx) -> Vec<String> {
        let mut vals = Vec::new();
        self.resolve(r, cx, &mut vals);
        let mut todo: Vec<Anchor> = vals
            .into_iter()
            .filter_map(|v| match v {
                Val::Anchor(a) => Some(a),
                _ => None,
            })
            .collect();
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        while let Some(a) = todo.pop() {
            if seen.len() >= 64 || !seen.insert(a.clone()) {
                continue;
            }
            if a.path.is_empty() {
                match &a.base {
                    Base::Class { name, .. } => out.push(name.clone()),
                    base => {
                        for (file, _) in self.anchor_files(&a) {
                            let Some(api) = self.api(file) else { continue };
                            for (owner, class) in &api.class_binds {
                                let same = match (base, owner) {
                                    (Base::Global { name, .. }, Owner::Global(g)) => name == g,
                                    (Base::Local { file: f, decl }, Owner::Local(d)) => f == file && decl == d,
                                    (Base::Table { file: f, id }, Owner::Table(t)) => f == file && id == t,
                                    _ => false,
                                };
                                if same {
                                    out.push(class.clone());
                                }
                            }
                        }
                    }
                }
            }
            todo.extend(self.aliases(&a, true));
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The class in `class`'s chain that declares `field` (`---@field`, or a
    /// value bound to the class), nearest first; `class` itself when none.
    fn field_class(&self, class: &str, field: &str, res: Option<usize>) -> String {
        self.field_owner(class, field, res).unwrap_or_else(|| class.to_string())
    }

    fn field_owner(&self, class: &str, field: &str, res: Option<usize>) -> Option<String> {
        let mut todo = std::collections::VecDeque::from([class.to_string()]);
        let mut seen = HashSet::new();
        while let Some(c) = todo.pop_front() {
            if seen.len() >= 32 || !seen.insert(c.clone()) {
                continue;
            }
            let defs = self.class_defs(&c, res);
            if defs.iter().any(|(d, _)| d.fields.iter().any(|f| f.name == field)) {
                return Some(c);
            }
            let anchor =
                Anchor { base: Base::Class { name: c.clone(), res, side: FileSide::Unknown }, path: Vec::new() };
            if self.defs(&anchor, true).iter().any(|(d, _)| d.path.last().is_some_and(|p| p == field)) {
                return Some(c);
            }
            for (d, _) in defs {
                todo.extend(d.parents.iter().cloned());
            }
        }
        None
    }

    /// The value a field site belongs to.
    fn holder_ref(&self, scan: &Scan, holder: &Holder) -> Option<Ref> {
        match holder {
            Holder::Expr(e) => {
                let (root, fields) = crate::assist::chain(&scan.text, &scan.names, e)?;
                Some(fields.into_iter().fold(root, |r, f| Ref::Field(Box::new(r), f)))
            }
            Holder::FuncName(n) => {
                let parts: Vec<_> = crate::util::tokens(n).filter(|t| t.kind() == NAME).collect();
                let (base, rest) = parts[..parts.len() - 1].split_first()?;
                let root = match scan.names.refs.binary_search_by_key(&base.text_range().start(), |x| x.range.start()) {
                    Ok(i) => match scan.names.refs[i].target {
                        crate::Target::Decl(d) => Ref::Local(d as u32),
                        crate::Target::Global => Ref::Global(base.text().to_string()),
                    },
                    Err(_) => Ref::Global(base.text().to_string()),
                };
                Some(rest.iter().fold(root, |r, t| Ref::Field(Box::new(r), t.text().to_string())))
            }
            Holder::Table(t) => table_ref(scan, t),
            Holder::Class(_) => None,
        }
    }

    /// The classes declaring the field at a site; empty when the value's
    /// class is not known.
    fn site_owners(&self, scan: &Scan, cx: &Cx, holder: &Holder, field: &str) -> Vec<String> {
        if let Holder::Class(c) = holder {
            return vec![c.clone()];
        }
        let mut classes = match self.holder_ref(scan, holder) {
            Some(r) => self.ref_classes(&r, cx),
            None => Vec::new(),
        };
        if classes.is_empty()
            && let Holder::Expr(e) = holder
        {
            classes = self.element_classes(scan, cx, e);
        }
        let mut out: Vec<String> = classes.iter().map(|c| self.field_class(c, field, cx.res)).collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Whether a site with no class is on a value known to be something else:
    /// a library table (`math.min`), a table the code builds (`local t = {}`,
    /// a constructor not passed or returned), a global the workspace defines.
    /// A parameter with no type, a call's result, a constructor handed to a
    /// call may still be a value of the class.
    fn site_known(&self, scan: &Scan, cx: &Cx, holder: &Holder, global_escapes: &dyn Fn(&str) -> bool) -> bool {
        match holder {
            Holder::Class(_) => true,
            Holder::Table(t) => {
                let handed = t.parent().is_some_and(|p| {
                    p.kind() == ARG_LIST
                        || (p.kind() == EXPR_LIST && p.parent().is_some_and(|s| s.kind() == RETURN_STAT))
                });
                if handed || matches!(t.parent().map(|p| p.kind()), Some(CALL_EXPR)) {
                    return false;
                }
                // Stored in a local: known while the local stays in the file.
                let mut r = table_ref(scan, t);
                while let Some(Ref::Field(base, _)) = r {
                    r = Some(*base);
                }
                match r {
                    Some(Ref::Local(d)) => !escapes(scan, d),
                    Some(Ref::Global(g)) => !global_escapes(&g),
                    _ => true,
                }
            }
            Holder::Expr(_) | Holder::FuncName(_) => {
                let Some(mut r) = self.holder_ref(scan, holder) else { return false };
                let mut nested = false;
                while let Ref::Field(base, _) = r {
                    r = *base;
                    nested = true;
                }
                // `State.cur.x`: the value stored in a field may be anything,
                // but a library's own tables (`math.min`).
                if nested {
                    return matches!(&r, Ref::Global(g) if !crate::library::library().get(g).is_empty());
                }
                match r {
                    Ref::Global(g) => {
                        !crate::library::library().get(&g).is_empty() || {
                            let a = Anchor {
                                base: Base::Global { res: cx.res, side: cx.side, name: g.clone() },
                                path: Vec::new(),
                            };
                            !self.defs(&a, false).is_empty() && !global_escapes(&g)
                        }
                    }
                    Ref::Local(d) => scan.names.decls.get(d as usize).is_some_and(|decl| {
                        (matches!(decl.value, crate::Value::Table | crate::Value::Function) && !escapes(scan, d))
                            || decl.kind == crate::DeclKind::SelfParam
                    }),
                    Ref::Table(_) | Ref::Require(_) | Ref::Export(..) => true,
                    _ => false,
                }
            }
        }
    }
}

impl<'a> Q<'a> {
    /// The classes of an element read from a `T[]` / `table<K, T>`: `xs[i]`,
    /// and `v` of `for _, v in ipairs(xs)` / `pairs(xs)`.
    fn element_classes(&self, scan: &Scan, cx: &Cx, e: &SyntaxNode) -> Vec<String> {
        let list = match e.kind() {
            INDEX_EXPR
                if crate::lint::field_name(e).is_none()
                    && e.children().nth(1).and_then(|k| crate::fields::string_key(&k)).is_none() =>
            {
                e.first_child()
            }
            // `local v = xs[i]`: the element its value is.
            NAME_EXPR => match local_init(scan, e) {
                Some(init) if init.kind() == INDEX_EXPR => return self.element_classes(scan, cx, &init),
                _ => for_list(scan, e),
            },
            _ => None,
        };
        let Some(list) = list else { return Vec::new() };
        let mut types = Vec::new();
        self.doc_types_of(scan, cx, &list, &mut types);
        let mut out = Vec::new();
        for t in types {
            element_names(&t, &mut out);
        }
        out.retain(|n| !builtin_type(n) && !self.doc_name_files(n, cx.res).is_empty());
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The doc types an expression is declared with: a local's or a
    /// parameter's `---@type` / `---@param`, a class field's `---@field`.
    fn doc_types_of(&self, scan: &Scan, cx: &Cx, e: &SyntaxNode, out: &mut Vec<crate::luacats::DocType>) {
        let Some((root, mut fields)) = crate::assist::chain(&scan.text, &scan.names, e) else { return };
        let Some(last) = fields.pop() else {
            if let Ref::Local(d) = root {
                for def in scan.api.defs.iter().filter(|x| x.owner == Owner::Local(d) && x.path.is_empty()) {
                    if let Ref::Doc(t) = &def.value {
                        out.push(t.clone());
                    }
                }
            }
            return;
        };
        let base = fields.into_iter().fold(root, |r, f| Ref::Field(Box::new(r), f));
        for c in self.ref_classes(&base, cx) {
            let owner = self.field_class(&c, &last, cx.res);
            out.extend(self.class_field_types(&owner, cx.res, &last, cx).into_iter().map(|(t, _)| t));
        }
    }
}

/// The names an element of the type is: `A` of `A[]`, `table<K, A>`, `list<A>`.
fn element_names(t: &crate::luacats::DocType, out: &mut Vec<String>) {
    use crate::luacats::DocType as D;
    let mut name = |t: &D| {
        let mut stack = vec![t];
        while let Some(t) = stack.pop() {
            match t {
                D::Name(n) => out.push(n.clone()),
                D::Optional(i) => stack.push(i),
                D::Union(ps) => stack.extend(ps.iter()),
                _ => {}
            }
        }
    };
    match t {
        D::Array(inner) => name(inner),
        D::Generic(_, args) if !args.is_empty() => name(&args[args.len() - 1]),
        D::Optional(inner) => element_names(inner, out),
        D::Union(ps) => ps.iter().for_each(|p| element_names(p, out)),
        _ => {}
    }
}

/// Whether a local's value leaves its statements: passed to a call, returned,
/// stored in another variable or in a table.
fn escapes(scan: &Scan, d: u32) -> bool {
    let root = scan.parse.syntax();
    scan.names.refs.iter().filter(|r| r.target == crate::Target::Decl(d as usize)).any(|r| ref_escapes(&root, r.range))
}

/// Whether the global `g` is handed on anywhere in the text (as `escapes`).
fn global_escapes_in(text: &str, g: &str) -> bool {
    let parse = crate::parse(text);
    let names = crate::names(&parse, text);
    let root = parse.syntax();
    names
        .refs
        .iter()
        .filter(|r| r.target == crate::Target::Global && text.get(std::ops::Range::<usize>::from(r.range)) == Some(g))
        .any(|r| ref_escapes(&root, r.range))
}

/// Whether the name read at `range` hands its value on.
fn ref_escapes(root: &SyntaxNode, range: TextRange) -> bool {
    let Some(e) = root.token_at_offset(range.start()).right_biased().and_then(|t| t.parent()) else {
        return false;
    };
    if e.kind() != NAME_EXPR {
        return false;
    }
    match e.parent().map(|p| (p.kind(), p)) {
        Some((ARG_LIST | FIELD_POS | FIELD_NAMED | FIELD_INDEXED, _)) => true,
        Some((EXPR_LIST, list)) => {
            list.parent().is_some_and(|s| matches!(s.kind(), RETURN_STAT | LOCAL_STAT | ASSIGN_STAT | GENERIC_FOR_STAT))
        }
        // `f "x"` / `f {}` carry no name; `t:m()` hands `t` to its own method.
        _ => false,
    }
}

/// The value a local is declared with (`local v = <init>`), when `e` names it.
fn local_init(scan: &Scan, e: &SyntaxNode) -> Option<SyntaxNode> {
    let t = e.first_token()?;
    let i = scan.names.refs.binary_search_by_key(&t.text_range().start(), |x| x.range.start()).ok()?;
    let crate::Target::Decl(d) = scan.names.refs[i].target else { return None };
    let decl = &scan.names.decls[d];
    if decl.kind != crate::DeclKind::Local {
        return None;
    }
    let stat = scan
        .parse
        .syntax()
        .token_at_offset(decl.range.start())
        .right_biased()?
        .parent_ancestors()
        .find(|a| a.kind() == LOCAL_STAT)?;
    let i =
        stat.children().filter(|c| c.kind() == LOCAL_NAME).position(|n| n.text_range().contains_range(decl.range))?;
    stat.children().find(|c| c.kind() == EXPR_LIST)?.children().nth(i)
}

/// The list `v` walks in `for _, v in ipairs(list)` / `pairs(list)`, when `e`
/// names that `v`.
fn for_list(scan: &Scan, e: &SyntaxNode) -> Option<SyntaxNode> {
    let t = e.first_token()?;
    let i = scan.names.refs.binary_search_by_key(&t.text_range().start(), |x| x.range.start()).ok()?;
    let crate::Target::Decl(d) = scan.names.refs[i].target else { return None };
    let decl = &scan.names.decls[d];
    if decl.kind != crate::DeclKind::ForVar {
        return None;
    }
    let stat = scan
        .parse
        .syntax()
        .token_at_offset(decl.range.start())
        .right_biased()?
        .parent_ancestors()
        .find(|a| a.kind() == GENERIC_FOR_STAT)?;
    let vars: Vec<_> =
        crate::util::tokens(&stat).take_while(|t| t.kind() != IN_KW).filter(|t| t.kind() == NAME).collect();
    if vars.get(1).map(|v| v.text_range()) != Some(decl.range) {
        return None;
    }
    let call = stat.children().find(|c| c.kind() == EXPR_LIST)?.first_child().filter(|c| c.kind() == CALL_EXPR)?;
    let callee = call.first_child().filter(|c| c.kind() == NAME_EXPR)?;
    if !matches!(callee.first_token()?.text(), "ipairs" | "pairs") {
        return None;
    }
    call.children().find(|c| c.kind() == ARG_LIST)?.first_child()
}

/// The value a table constructor is stored in: `local t = {}`, `t = {}`, a
/// key of an outer constructor.
fn table_ref(scan: &Scan, table: &SyntaxNode) -> Option<Ref> {
    let parent = table.parent()?;
    match parent.kind() {
        EXPR_LIST => {
            let i = parent.children().position(|c| c == *table)?;
            let stat = parent.parent()?;
            match stat.kind() {
                LOCAL_STAT => {
                    let name = stat.children().filter(|c| c.kind() == LOCAL_NAME).nth(i)?;
                    let tok = crate::util::tokens(&name).find(|t| t.kind() == NAME)?;
                    let d = scan.names.decls.iter().position(|d| d.range == tok.text_range())?;
                    Some(Ref::Local(d as u32))
                }
                ASSIGN_STAT => {
                    let target = stat.children().find(|c| c.kind() == VAR_LIST)?.children().nth(i)?;
                    let (root, fields) = crate::assist::chain(&scan.text, &scan.names, &target)?;
                    Some(fields.into_iter().fold(root, |r, f| Ref::Field(Box::new(r), f)))
                }
                _ => None,
            }
        }
        FIELD_NAMED => {
            let k = crate::util::tokens(&parent).find(|t| t.kind() == NAME)?;
            let outer = parent.parent()?;
            Some(Ref::Field(Box::new(table_ref(scan, &outer)?), k.text().to_string()))
        }
        _ => None,
    }
}

impl Workspace {
    /// The class, alias, enum or class field at `offset`, when the file's
    /// types name one.
    pub fn type_target(&self, path: &Path, text: &str, offset: TextSize) -> Option<TypeTarget> {
        self.type_target_at(path, text, offset).map(|(t, _)| t)
    }

    /// `type_target` and the range of the name at `offset` (a string key's
    /// content).
    pub fn type_target_at(&self, path: &Path, text: &str, offset: TextSize) -> Option<(TypeTarget, TextRange)> {
        let scan = Scan::new(path, text.to_string());
        let at = |r: &TextRange| r.contains_inclusive(offset);
        // A doc name.
        let root = scan.parse.syntax();
        if root.token_at_offset(offset).any(|t| matches!(t.kind(), COMMENT | BLOCK_COMMENT)) {
            for g in crate::lint::code_doc_groups(&root, text) {
                let generic = |n: &str| {
                    g.tags.iter().any(|t| match &t.tag {
                        Tag::Generic { names } => names.iter().any(|(g, _)| g == n),
                        Tag::Class { generics, .. } | Tag::Alias { generics, .. } => generics.iter().any(|g| g == n),
                        _ => false,
                    })
                };
                for t in &g.tags {
                    let declared = match &t.tag {
                        Tag::Class { .. } | Tag::Alias { .. } | Tag::Enum { .. } => t.name.filter(at),
                        _ => None,
                    };
                    if let Some(r) = declared.or_else(|| t.types.iter().copied().find(at)) {
                        let name = text.get(std::ops::Range::<usize>::from(r))?.to_string();
                        return (!builtin_type(&name) && !generic(&name)).then_some((TypeTarget::DocName(name), r));
                    }
                }
            }
            for tok in root
                .descendants_with_tokens()
                .filter_map(|e| e.into_token())
                .filter(|t| matches!(t.kind(), COMMENT | BLOCK_COMMENT) && at(&t.text_range()))
            {
                if let Some(r) =
                    crate::luacats::inline_as_types(tok.text(), tok.text_range().start()).into_iter().find(at)
                {
                    let name = text.get(std::ops::Range::<usize>::from(r))?.to_string();
                    return (!builtin_type(&name)).then_some((TypeTarget::DocName(name), r));
                }
            }
        }
        // A field.
        let tok = root.token_at_offset(offset).find(|t| matches!(t.kind(), NAME | STRING | LONG_STRING | COMMENT))?;
        let name = match tok.kind() {
            NAME => tok.text().to_string(),
            STRING | LONG_STRING => crate::fields::string_key(&tok.parent()?)?.0,
            _ => {
                // `---@field x`.
                let g = crate::lint::code_doc_groups(&root, text)
                    .into_iter()
                    .find(|g| g.tags.iter().any(|t| at(&t.range)))?;
                let t = g.tags.iter().find(|t| {
                    matches!(t.tag, Tag::Field { key: Key::Name(_), .. }) && t.name.is_some_and(|r| at(&r))
                })?;
                text.get(std::ops::Range::<usize>::from(t.name?))?.to_string()
            }
        };
        let (range, holder) = field_sites(&scan, &name).into_iter().find(|(r, _)| at(r))?;
        let q = Q::new(self, Some((scan.key.as_str(), &scan.api)));
        let cx = q.cx_of(&scan.key, FileSide::Unknown);
        let owner = q.site_owners(&scan, &cx, &holder, &name).into_iter().next()?;
        Some((TypeTarget::Field { owner, name }, range))
    }

    /// Every place naming the target, in the files that see the same
    /// declarations as `path`. `text_of`: a file's text (the open document
    /// over the disk).
    pub fn type_references(
        &self,
        path: &Path,
        target: &TypeTarget,
        text_of: &dyn Fn(&Path) -> Option<String>,
    ) -> FileRanges {
        self.search(path, target, text_of).ranges
    }

    /// The edits renaming the target, or why it cannot be renamed: a library
    /// or builtin name, an invalid or taken new name, and for a field any
    /// same-named access whose value's class is not known in a file that sees
    /// the class, or an escrow resource that may read it.
    pub fn type_rename(
        &self,
        path: &Path,
        target: &TypeTarget,
        new_name: &str,
        text_of: &dyn Fn(&Path) -> Option<String>,
    ) -> Result<FileRanges, String> {
        let q = Q::new(self, None);
        let k = key(path);
        let res = q.cx_of(&k, FileSide::Unknown).res;
        match target {
            TypeTarget::DocName(name) => {
                // A name no file declares is renamed where it is written (LuaLS).
                if q.doc_name_files(name, res).is_empty()
                    && (crate::library::doc_names().contains(name.as_str())
                        || crate::library::classes().contains_key(name.as_str()))
                {
                    return Err(format!("`{name}` comes from the library"));
                }
                let valid = new_name.split('.').all(|part| {
                    part.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                        && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                });
                if !valid {
                    return Err(format!("`{new_name}` is not a valid type name"));
                }
                if builtin_type(new_name)
                    || !q.doc_name_files(new_name, res).is_empty()
                    || crate::library::doc_names().contains(new_name)
                {
                    return Err(format!("a type named `{new_name}` already exists"));
                }
            }
            TypeTarget::Field { owner, .. } => {
                if !crate::names::is_name(new_name) {
                    return Err(format!("`{new_name}` is not a valid Lua name"));
                }
                if let Some(c) = q.field_owner(owner, new_name, res) {
                    return Err(format!("`{c}` already has a field `{new_name}`"));
                }
                let files = q.doc_name_files(owner, res);
                if files.is_empty() {
                    return Err(format!("`{owner}` is not declared in the workspace"));
                }
                if self.doc_types_hidden(path)
                    || files.iter().any(|f| self.files.get(*f).is_some_and(|e| self.doc_types_hidden(&e.path)))
                {
                    return Err("an escrow resource may use this field: it cannot be renamed".into());
                }
            }
        }
        let found = self.search(path, target, text_of);
        if found.exhausted {
            return Err("the types are too deep to follow: not renamed".into());
        }
        if let Some((file, at)) = found.unknown.first() {
            return Err(format!(
                "{} use(s) of `{}` on a value of unknown type (first: {}, offset {}): not renamed",
                found.unknown.len(),
                match target {
                    TypeTarget::Field { name, .. } | TypeTarget::DocName(name) => name,
                },
                file.display(),
                u32::from(*at)
            ));
        }
        if let Some((file, at)) = found.ambiguous.first() {
            return Err(format!(
                "a value at {}, offset {} may be of another class with this field: not renamed",
                file.display(),
                u32::from(*at)
            ));
        }
        if found.ranges.is_empty() {
            return Err("nothing to rename".into());
        }
        Ok(found.ranges)
    }

    fn search(&self, path: &Path, target: &TypeTarget, text_of: &dyn Fn(&Path) -> Option<String>) -> Found {
        let mut found = Found::default();
        let name = match target {
            TypeTarget::DocName(n) => n.as_str(),
            TypeTarget::Field { owner, .. } => owner.as_str(),
        };
        let here = key(path);
        let base = Q::new(self, None);
        // The declaring files each resource sees, once per resource.
        let mut scopes: HashMap<Option<usize>, Vec<String>> = HashMap::new();
        let mut scope = |res: Option<usize>| -> Vec<String> {
            scopes
                .entry(res)
                .or_insert_with(|| base.doc_name_files(name, res).into_iter().map(String::from).collect())
                .clone()
        };
        let res_of = |k: &str| base.cx_of(k, FileSide::Unknown).res;
        let want = scope(res_of(&here));
        // The cursor's file first, then the others by path.
        let mut paths: Vec<PathBuf> = self.files.values().map(|e| e.path.clone()).filter(|p| key(p) != here).collect();
        paths.sort();
        paths.insert(0, path.to_path_buf());
        let word = match target {
            TypeTarget::DocName(n) | TypeTarget::Field { name: n, .. } => n.as_str(),
        };
        // A field is looked for in the resources that name its class (or
        // declare it): a value of the class reaches others only untyped.
        let mut naming: HashSet<Option<usize>> = want.iter().map(|f| res_of(f)).collect();
        let mut texts = Vec::new();
        // For a field, every file's text: a global handed on anywhere in the
        // resources searched may be a value of the class.
        let mut all: Vec<(String, Option<usize>)> = Vec::new();
        let field = matches!(target, TypeTarget::Field { .. });
        for p in paths {
            let Some(text) = text_of(&p) else { continue };
            let k = key(&p);
            let res = res_of(&k);
            if field && has_word(&text, name) {
                naming.insert(res);
            }
            if !has_word(&text, word) || scope(res) != want {
                if field {
                    all.push((text, res));
                }
                continue;
            }
            texts.push((p, text, res));
        }
        let escaping: std::cell::RefCell<HashMap<String, bool>> = Default::default();
        let global_escapes = |g: &str| -> bool {
            if let Some(&e) = escaping.borrow().get(g) {
                return e;
            }
            let e = all
                .iter()
                .map(|(t, r)| (t.as_str(), r))
                .chain(texts.iter().map(|(_, t, r)| (t.as_str(), r)))
                .filter(|(_, r)| naming.contains(r))
                .any(|(t, _)| has_word(t, g) && global_escapes_in(t, g));
            escaping.borrow_mut().insert(g.to_string(), e);
            e
        };
        for (p, text, res) in texts.iter().cloned() {
            if matches!(target, TypeTarget::Field { .. }) && !naming.contains(&res) {
                continue;
            }
            let scan = Scan::new(&p, text);
            let mut ranges = Vec::new();
            match target {
                TypeTarget::DocName(n) => ranges.extend(doc_name_sites(&scan, n).into_iter().map(|(r, _)| r)),
                TypeTarget::Field { owner, name } => {
                    let q = Q::new(self, Some((scan.key.as_str(), &scan.api)));
                    let cx = q.cx_of(&scan.key, FileSide::Unknown);
                    for (r, holder) in field_sites(&scan, name) {
                        q.budget.set(super::BUDGET);
                        let owners = q.site_owners(&scan, &cx, &holder, name);
                        if q.budget.get() == 0 {
                            found.exhausted = true;
                        }
                        if owners.iter().any(|o| o == owner) {
                            // `---@type A|B`: renaming A's field would break B.
                            if owners.len() > 1 {
                                found.ambiguous.push((scan.path.clone(), r.start()));
                            }
                            ranges.push(r);
                        } else if owners.is_empty() && !q.site_known(&scan, &cx, &holder, &global_escapes) {
                            found.unknown.push((scan.path.clone(), r.start()));
                        }
                    }
                }
            }
            if !ranges.is_empty() {
                ranges.sort_by_key(|r| r.start());
                ranges.dedup();
                found.ranges.push((scan.path, ranges));
            }
        }
        found
    }
}

/// `w` in `text` as a whole word.
fn has_word(text: &str, w: &str) -> bool {
    let ident = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
    text.match_indices(w)
        .any(|(i, _)| !ident(text[..i].chars().next_back()) && !ident(text[i + w.len()..].chars().next()))
}

#[derive(Default)]
struct Found {
    ranges: FileRanges,
    /// Field accesses of the name whose value's class is not known.
    unknown: Vec<(PathBuf, TextSize)>,
    /// Sites on a value that may also be of another class declaring the field.
    ambiguous: Vec<(PathBuf, TextSize)>,
    exhausted: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, rel: &str, text: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn read(p: &Path) -> Option<String> {
        std::fs::read_to_string(p).ok()
    }

    /// (file name, text) of every range found.
    fn found(root: &Path, ranges: &FileRanges) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for (p, rs) in ranges {
            let text = std::fs::read_to_string(p).unwrap();
            let name = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
            out.extend(rs.iter().map(|r| (name.clone(), text[*r].to_string())));
        }
        out.sort();
        out
    }

    fn at(ws_file: &Path, needle: &str) -> TextSize {
        let text = std::fs::read_to_string(ws_file).unwrap();
        TextSize::from(text.find(needle).unwrap() as u32)
    }

    #[test]
    fn across_files_and_resources() {
        let root = std::env::temp_dir().join(format!("cfx-typerefs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // Two resources with their own `Config` class; a third reads `a`'s.
        write(&root, "a/fxmanifest.lua", "shared_script 'config.lua'\nclient_script 'client.lua'\n");
        write(&root, "a/config.lua", "---@class Config\n---@field speed number\nConfig = {}\nConfig.speed = 1\n");
        write(
            &root,
            "a/client.lua",
            "---@type Config\nlocal c = Config\nprint(c.speed, Config['speed'])\n---@param cfg Config\nlocal function f(cfg) return cfg.speed end\n",
        );
        write(&root, "b/fxmanifest.lua", "shared_script 'config.lua'\n");
        write(&root, "b/config.lua", "---@class Config\n---@field speed number\nConfig = {}\nprint(Config.speed)\n");
        let ws = Workspace::scan(std::slice::from_ref(&root));
        let file = root.join("a/client.lua");
        let text = std::fs::read_to_string(&file).unwrap();
        let target = ws.type_target(&file, &text, at(&file, "speed,")).unwrap();
        assert_eq!(target, TypeTarget::Field { owner: "Config".into(), name: "speed".into() });
        let refs = ws.type_references(&file, &target, &read);
        let want = [
            ("a/client.lua", "speed"),
            ("a/client.lua", "speed"),
            ("a/client.lua", "speed"),
            ("a/config.lua", "speed"),
            ("a/config.lua", "speed"),
        ];
        assert_eq!(found(&root, &refs), want.map(|(a, b)| (a.to_string(), b.to_string())));
        // The same from b's own class: only b.
        let bfile = root.join("b/config.lua");
        let btext = std::fs::read_to_string(&bfile).unwrap();
        let bt = ws.type_target(&bfile, &btext, at(&bfile, "speed)")).unwrap();
        let brefs = ws.type_references(&bfile, &bt, &read);
        assert!(found(&root, &brefs).iter().all(|(f, _)| f == "b/config.lua"), "{brefs:?}");
        // The class name: declarations and uses in a's files.
        let ct = ws.type_target(&file, &text, at(&file, "Config\nlocal")).unwrap();
        assert_eq!(ct, TypeTarget::DocName("Config".into()));
        let names = found(&root, &ws.type_references(&file, &ct, &read));
        assert_eq!(
            names.iter().map(|(f, _)| f.as_str()).collect::<Vec<_>>(),
            ["a/client.lua", "a/client.lua", "a/config.lua"]
        );
        // Rename moves every one of them, the string key's content too.
        let edits = ws.type_rename(&file, &target, "pace", &read).unwrap();
        assert_eq!(edits.iter().map(|(_, r)| r.len()).sum::<usize>(), 5);
        assert!(ws.type_rename(&file, &target, "not a name", &read).is_err());
        assert!(ws.type_rename(&file, &ct, "number", &read).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn elements_loops_and_known_tables() {
        let root = std::env::temp_dir().join(format!("cfx-typerefs3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        write(&root, "a/fxmanifest.lua", "client_script '*.lua'\n");
        let text = "---@class Ring\n---@field min number\n\n---@param rings Ring[]\n---@param byKey table<string, Ring>\nlocal function f(rings, byKey)\n    for _, r in ipairs(rings) do print(r.min) end\n    for _, r in pairs(byKey) do print(r.min) end\n    local first = rings[1]\n    print(first.min, rings[2].min, math.min(1, 2))\n    local t = { min = 1 }\n    return t.min\nend\nreturn f\n";
        write(&root, "a/a.lua", text);
        let ws = Workspace::scan(std::slice::from_ref(&root));
        let file = root.join("a/a.lua");
        let target = ws.type_target(&file, text, at(&file, "min number")).unwrap();
        let refs = ws.type_references(&file, &target, &read);
        // The field, both loops, the local, the index: not `math.min` or `t`.
        assert_eq!(refs.iter().map(|(_, r)| r.len()).sum::<usize>(), 5, "{refs:?}");
        assert_eq!(
            ws.type_rename(&file, &target, "low", &read).unwrap().iter().map(|(_, r)| r.len()).sum::<usize>(),
            5
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// One file `a/a.lua` in a fresh workspace.
    fn one(text: &str) -> (PathBuf, Workspace, PathBuf) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "cfx-typerefs-one-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        write(&root, "a/fxmanifest.lua", "client_script '*.lua'\n");
        write(&root, "a/a.lua", text);
        let ws = Workspace::scan(std::slice::from_ref(&root));
        let file = root.join("a/a.lua");
        (root, ws, file)
    }

    /// The texts of the references of what is at the `n`th `needle`.
    fn refs_at(text: &str, needle: &str, n: usize) -> (Vec<String>, Option<TypeTarget>) {
        let (root, ws, file) = one(text);
        let at = text.match_indices(needle).nth(n).unwrap().0 + 1;
        let target = ws.type_target(&file, text, TextSize::from(at as u32));
        let out = target
            .as_ref()
            .map(|t| {
                ws.type_references(&file, t, &read)
                    .into_iter()
                    .flat_map(|(_, r)| r)
                    .map(|r| text[r].to_string())
                    .collect()
            })
            .unwrap_or_default();
        std::fs::remove_dir_all(&root).unwrap();
        (out, target)
    }

    fn rename_at(text: &str, needle: &str, new: &str) -> Result<usize, String> {
        let (root, ws, file) = one(text);
        let at = text.find(needle).unwrap() + 1;
        let target = ws.type_target(&file, text, TextSize::from(at as u32)).expect("a target");
        let r = ws.type_rename(&file, &target, new, &read).map(|f| f.iter().map(|(_, r)| r.len()).sum());
        std::fs::remove_dir_all(&root).unwrap();
        r
    }

    #[test]
    fn doc_types_over_lines_and_casts() {
        // Continued lines keep their own offsets, multi-byte text too.
        let text = "---@class Foo\n---@type {\n---  a: Foo,\n---  b: Foo }\nlocal x\n---@type fun(\n---  ğğğ: Foo)\nlocal f\nlocal y = x --[[@as Foo]]\n";
        let (got, _) = refs_at(text, "Foo", 0);
        assert_eq!(got, ["Foo"; 5]);
        // The cast is a target too; a name in the middle of a table type is not.
        assert_eq!(refs_at(text, "Foo]]", 0).1, Some(TypeTarget::DocName("Foo".into())));
        assert_eq!(refs_at(text, "a: Foo", 0).1, None);
        // A generic is not the class of its name.
        let text = "---@class T\n---@generic T\n---@param x T\nlocal function f(x) end\n";
        assert_eq!(refs_at(text, "T\nlocal", 0).1, None);
        assert!(rename_at("---@class A\nlocal a = {}\n", "A\n", "A..B").is_err());
    }

    #[test]
    fn field_rename_refusals() {
        let class = "---@class Cfg\n---@field speed number\n---@field pace number\n\n---@param c Cfg\nfunction Use(c) return c.speed end\n";
        // A taken name.
        assert!(rename_at(class, "speed number", "pace").unwrap_err().contains("already has"));
        // The value stored in a field may be a `Cfg`.
        let nested = format!("{class}State = {{}}\nfunction Load(x) State.cur = x end\nprint(State.cur.speed)\n");
        assert!(rename_at(&nested, "speed number", "rate").is_err());
        // A local table handed to a function may become one.
        let handed = format!("{class}local t = {{ speed = 1 }}\nUse(t)\n");
        assert!(rename_at(&handed, "speed number", "rate").is_err());
        let kept = format!("{class}local t = {{ speed = 1 }}\nprint(t.speed)\n");
        assert_eq!(rename_at(&kept, "speed number", "rate"), Ok(2));
        // A global table handed to a function may be one too.
        let global = format!(
            "{class}Config = {{ speed = 1 }}
Use(Config)
"
        );
        assert!(rename_at(&global, "speed number", "rate").is_err());
        let kept_global = format!(
            "{class}Config = {{ speed = 1 }}
print(Config.speed)
"
        );
        assert_eq!(rename_at(&kept_global, "speed number", "rate"), Ok(2));
        // A string key of a typed constructor.
        let keyed = format!("{class}---@type Cfg\nlocal k = {{ [\"speed\"] = 1 }}\n");
        assert_eq!(rename_at(&keyed, "speed number", "rate"), Ok(3));
        // `A|B` where both declare the field.
        let union = format!(
            "{class}---@class Other\n---@field speed number\nOther = {{}}\n\n---@type Cfg|Other\nlocal ab\nprint(ab.speed)\n"
        );
        assert!(rename_at(&union, "speed number", "rate").unwrap_err().contains("another class"));
    }

    #[test]
    fn rename_refuses_unknown_uses() {
        let root = std::env::temp_dir().join(format!("cfx-typerefs2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        write(&root, "a/fxmanifest.lua", "shared_script '*.lua'\n");
        write(
            &root,
            "a/a.lua",
            "---@class Item\n---@field label string\n\n---@type Item\nlocal item\nprint(item.label)\n",
        );
        // A value of no known type reads the same name: it may be an `Item`.
        write(&root, "a/b.lua", "local function show(x) return x.label end\nreturn show\n");
        let ws = Workspace::scan(std::slice::from_ref(&root));
        let file = root.join("a/a.lua");
        let text = std::fs::read_to_string(&file).unwrap();
        let target = ws.type_target(&file, &text, at(&file, "label)")).unwrap();
        assert_eq!(ws.type_references(&file, &target, &read).len(), 1);
        let err = ws.type_rename(&file, &target, "title", &read).unwrap_err();
        assert!(err.contains("unknown type"), "{err}");
        // A library class is not ours to rename.
        write(&root, "a/c.lua", "---@type vector3\nlocal v\nprint(v.x)\n");
        let ws = Workspace::scan(std::slice::from_ref(&root));
        let c = root.join("a/c.lua");
        let ctext = std::fs::read_to_string(&c).unwrap();
        if let Some(t) = ws.type_target(&c, &ctext, at(&c, "x)")) {
            assert!(ws.type_rename(&c, &t, "y", &read).is_err());
        }
        std::fs::remove_dir_all(&root).unwrap();
    }
}
