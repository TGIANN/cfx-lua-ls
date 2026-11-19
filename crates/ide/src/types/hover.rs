//! Hover for globals, fields and the functions they hold, as LuaLS writes it
//! (`core/hover/label.lua`, `name.lua`): `(global) a.b: integer = 600`,
//! `(field) t.v: {`, `function obj.f()`, `(method) mt:init(a: any)`. What
//! a path holds comes from every place of the file that writes or reads it.

use super::*;
use crate::luacats::Visibility;

#[derive(Clone, PartialEq, Debug)]
enum Root {
    Decl(usize),
    Global(String),
}

#[derive(Clone, PartialEq, Debug)]
struct Path {
    root: Root,
    fields: Vec<String>,
}

impl Path {
    fn child(&self, k: &str) -> Path {
        let mut fields = self.fields.clone();
        fields.push(k.to_string());
        Path { root: self.root.clone(), fields }
    }

    /// `self` is `p` with one more key: that key.
    fn key_under(&self, p: &Path) -> Option<&str> {
        (self.root == p.root && self.fields.len() == p.fields.len() + 1 && self.fields.starts_with(&p.fields)).then(|| self.fields.last().unwrap().as_str())
    }

    fn starts(&self, p: &Path) -> bool {
        self.root == p.root && p.fields.len() > self.fields.len() && p.fields.starts_with(&self.fields)
    }
}

/// A value the path is given.
struct Occ {
    value: Option<SyntaxNode>,
    /// `---@type T` / `---@class C` on the statement.
    doc: Option<T>,
    own: Option<String>,
    /// `function a.b()`: its statement.
    function: Option<SyntaxNode>,
}

impl FileTypes<'_> {
    fn root_of(&self, tok: &SyntaxToken) -> Option<Root> {
        match self.inf.ref_at(tok.text_range()) {
            Some(r) => Some(match r.target {
                Target::Decl(d) => Root::Decl(d),
                Target::Global => Root::Global(tok.text().to_string()),
            }),
            None => self.inf.decl_at(tok.text_range()).map(Root::Decl),
        }
    }

    fn path_of(&self, e: &SyntaxNode) -> Option<Path> {
        match e.kind() {
            NAME_EXPR => Some(Path { root: self.root_of(&e.first_token()?)?, fields: Vec::new() }),
            INDEX_EXPR => {
                let mut p = self.path_of(&e.first_child()?)?;
                p.fields.push(index_key(e)?);
                Some(p)
            }
            PAREN_EXPR => self.path_of(&e.first_child()?),
            _ => None,
        }
    }

    /// The path of the first `n` names of `function a.b:c()`.
    fn func_name_path(&self, fname: &SyntaxNode, n: usize) -> Option<Path> {
        let names = func_names(fname);
        let first = names.first()?;
        Some(Path { root: self.root_of(first)?, fields: names[1..n.min(names.len())].iter().map(|t| t.text().to_string()).collect() })
    }

    /// The values the file gives `p` and the fields written or read under it.
    fn scan(&self, p: &Path) -> (Vec<Occ>, Shape) {
        let inf = &self.inf;
        let mut occs = Vec::new();
        let mut shape = Shape::default();
        for n in inf.root.descendants() {
            match n.kind() {
                ASSIGN_STAT | LOCAL_STAT => {
                    let tags = stat_tags(inf.text, &n);
                    let doc = tags.iter().find_map(|t| match t {
                        Tag::Type { tys } => tys.first().cloned(),
                        Tag::Class { name: c, .. } => Some(name(c)),
                        _ => None,
                    });
                    let own = tags.iter().rev().find_map(|t| match t {
                        Tag::Class { name, .. } => Some(name.clone()),
                        _ => None,
                    });
                    let vis = tags.iter().find_map(|t| match t {
                        Tag::Visibility(v) => Some(*v),
                        _ => None,
                    });
                    let values: Vec<SyntaxNode> = n.children().find(|c| c.kind() == EXPR_LIST).map(|l| l.children().collect()).unwrap_or_default();
                    let targets: Vec<Option<Path>> = if n.kind() == ASSIGN_STAT {
                        n.children().find(|c| c.kind() == VAR_LIST).map(|l| l.children().map(|v| self.path_of(&v)).collect()).unwrap_or_default()
                    } else {
                        n.children()
                            .filter(|c| c.kind() == LOCAL_NAME)
                            .map(|l| l.first_token().and_then(|t| inf.decl_at(t.text_range())).map(|d| Path { root: Root::Decl(d), fields: Vec::new() }))
                            .collect()
                    };
                    for (i, target) in targets.into_iter().enumerate() {
                        let Some(vp) = target else { continue };
                        let value = values.get(i).cloned();
                        let doc = if i == 0 { doc.clone() } else { None };
                        self.walk_value(p, &vp, value, doc, own.clone(), vis, &mut occs, &mut shape);
                    }
                }
                FUNCTION_STAT => {
                    let Some(fname) = n.children().find(|c| c.kind() == FUNC_NAME) else { continue };
                    let Some(fp) = self.func_name_path(&fname, usize::MAX) else { continue };
                    if fp == *p {
                        occs.push(Occ { value: None, doc: None, own: None, function: Some(n.clone()) });
                    } else if let Some(k) = fp.key_under(p) {
                        let vis = stat_tags(inf.text, &n).into_iter().find_map(|t| match t {
                            Tag::Visibility(v) => Some(v),
                            _ => None,
                        });
                        let e = shape.entry(k.to_string());
                        e.types.push(name("function"));
                        e.visibility = e.visibility.or(vis);
                    }
                }
                INDEX_EXPR => {
                    if let Some(k) = self.path_of(&n).as_ref().and_then(|vp| vp.key_under(p)) {
                        shape.entry(k.to_string());
                    }
                }
                METHOD_CALL_EXPR => {
                    if let (Some(obj), Some(m)) = (n.first_child(), crate::api::method_name(&n))
                        && self.path_of(&obj).as_ref() == Some(p)
                    {
                        shape.entry(m.text().to_string());
                    }
                }
                _ => {}
            }
        }
        (occs, shape)
    }

    /// `vp = value`: a value of `p`, a field of it, or a constructor holding
    /// either deeper down.
    #[allow(clippy::too_many_arguments)]
    fn walk_value(&self, p: &Path, vp: &Path, value: Option<SyntaxNode>, doc: Option<T>, own: Option<String>, vis: Option<Visibility>, occs: &mut Vec<Occ>, shape: &mut Shape) {
        if vp == p {
            if let Some(t) = value.as_ref().filter(|v| v.kind() == TABLE_EXPR) {
                self.constructor_shape(t, shape);
            }
            occs.push(Occ { value, doc, own, function: None });
        } else if let Some(k) = vp.key_under(p) {
            let e = shape.entry(k.to_string());
            e.visibility = e.visibility.or(vis);
            match (doc, value) {
                (Some(t), _) => e.types.push(t),
                (None, Some(v)) => e.values.push(v),
                (None, None) => {}
            }
        } else if vp.starts(p)
            && let Some(table) = value.filter(|v| v.kind() == TABLE_EXPR)
        {
            for f in table.children().filter(|f| f.kind() == FIELD_NAMED) {
                let Some(k) = f.children_with_tokens().filter_map(|x| x.into_token()).find(|t| t.kind() == NAME) else { continue };
                self.walk_value(p, &vp.child(k.text()), f.children().last(), None, None, None, occs, shape);
            }
        }
    }

    /// The code block of a hover on a global, a field or a function of the
    /// file, and the statement that defines it (for its doc comment); None for
    /// locals, what this does not cover, and (unless `force`) a path the file
    /// gives no value.
    pub(crate) fn path_hover(&self, tok: &SyntaxToken, force: bool) -> Option<(String, Option<TextRange>)> {
        let inf = &self.inf;
        let parent = tok.parent()?;
        // (path, `(global)` or `(field)`, the name shown, the expression).
        let (p, global, shown, expr) = match parent.kind() {
            NAME_EXPR => {
                let p = self.path_of(&parent)?;
                let Root::Global(g) = &p.root else { return None };
                (p.clone(), true, g.clone(), Some(parent.clone()))
            }
            INDEX_EXPR if crate::lint::field_name(&parent).as_ref() == Some(tok) => {
                let obj = parent.first_child()?;
                let p = self.path_of(&parent);
                let global = p.as_ref().is_some_and(|p| matches!(p.root, Root::Global(_)));
                let owner = if global { None } else { self.class_view(&obj) };
                let owner = owner.or_else(|| p.as_ref().map(|p| self.path_text(&Path { root: p.root.clone(), fields: p.fields[..p.fields.len() - 1].to_vec() })))?;
                let p = p.unwrap_or(Path { root: Root::Global(String::new()), fields: vec![String::new()] });
                (p, global, format!("{owner}.{}", tok.text()), Some(parent.clone()))
            }
            METHOD_CALL_EXPR => {
                let obj = parent.first_child()?;
                let m = crate::api::method_name(&parent).filter(|m| m == tok)?;
                let p = self.method_target(&obj)?.child(m.text());
                let (occs, _) = self.scan(&p);
                return occs.iter().find_map(|o| o.function.as_ref().map(|f| (self.function_label(f), Some(f.text_range()))));
            }
            FUNC_NAME => {
                let names = func_names(&parent);
                let i = names.iter().position(|t| t == tok)?;
                if i + 1 == names.len() {
                    let stat = parent.parent()?;
                    return Some((self.function_label(&stat), Some(stat.text_range())));
                }
                let p = self.func_name_path(&parent, i + 1)?;
                let global = matches!(p.root, Root::Global(_));
                if !global && p.fields.is_empty() {
                    return None;
                }
                (p.clone(), global, self.path_text(&p), None)
            }
            FIELD_NAMED => {
                let value = parent.children().last()?;
                let ty = if value.kind() == FUNCTION_EXPR { "function".to_string() } else { crate::luacats::view(&widen(&inf.expr_type(&value))) };
                // A constructor's entries take the place of `table`.
                let mut shape = Shape::default();
                if value.kind() == TABLE_EXPR {
                    self.constructor_shape(&value, &mut shape);
                }
                let fields = shape.lines(self);
                let mut code = if fields.is_some() { format!("(field) {}:", tok.text()) } else { format!("(field) {}: {ty}", tok.text()) };
                if let Some(l) = crate::consts::literals_shown(&inf.root, inf.names, inf.text, std::slice::from_ref(&value)) {
                    code.push_str(" = ");
                    code.push_str(&l);
                }
                push_fields(&mut code, fields);
                return Some((code, None));
            }
            _ => return None,
        };
        let (occs, mut shape) = if p.fields.first().is_some_and(String::is_empty) { (Vec::new(), Shape::default()) } else { self.scan(&p) };
        if !force && occs.is_empty() {
            return None;
        }
        // A function: its signature.
        if !occs.is_empty() && occs.iter().all(|o| o.doc.is_none() && (o.function.is_some() || o.value.as_ref().is_some_and(|v| v.kind() == FUNCTION_EXPR))) {
            if let Some(f) = occs.iter().find_map(|o| o.function.as_ref()) {
                return Some((self.function_label(f), Some(f.text_range())));
            }
            if let Some(v) = occs.iter().find_map(|o| o.value.as_ref())
                && let Some(body) = v.children().find(|c| c.kind() == FUNC_BODY)
            {
                let (sig, rets, is_async) = self.signature(&body);
                let stat = v.ancestors().find(|a| matches!(a.kind(), ASSIGN_STAT | LOCAL_STAT)).map(|s| s.text_range());
                return Some((function_code(is_async, "function", &shown, &sig, &rets), stat));
            }
        }
        let mut types: Vec<T> = Vec::new();
        let mut values = Vec::new();
        let mut own = None;
        for o in &occs {
            own = own.or(o.own.clone());
            match (&o.doc, &o.value, &o.function) {
                (Some(t), _, _) => types.push(t.clone()),
                (None, _, Some(_)) => types.push(name("function")),
                (None, Some(v), _) => {
                    types.push(if v.kind() == FUNCTION_EXPR { name("function") } else { widen(&inf.expr_type(v)) });
                    values.push(v.clone());
                }
                _ => {}
            }
        }
        types.retain(|t| !is(t, "nil") || occs.len() == 1);
        if types.is_empty()
            && let Some(e) = &expr
        {
            types.push(inf.expr_type(e));
        }
        let ty = if types.is_empty() { unknown() } else { union(types) };
        let t = inf.expand_aliases(&non_nil(ty.clone()), 0);
        let view = crate::luacats::view(&ty);
        if !(is(&t, "string") || matches!(t, T::Str(_))) {
            self.type_shape(&t, own.as_deref(), &mut shape, None);
        }
        let literal = crate::consts::literals_shown(&inf.root, inf.names, inf.text, &values);
        let fields = if is(&t, "string") || matches!(t, T::Str(_)) { None } else { shape.lines(self) };
        let title = if global { "(global)" } else { "(field)" };
        let mut code = format!("{title} {shown}:");
        if !(fields.is_some() && matches!(view.as_str(), "table" | "any" | "unknown" | "nil")) {
            code.push(' ');
            code.push_str(&view);
        }
        if let Some(l) = literal {
            code.push_str(" = ");
            code.push_str(&l);
        }
        push_fields(&mut code, fields);
        let def = occs.iter().find_map(|o| o.value.as_ref()).and_then(|v| v.ancestors().find(|a| matches!(a.kind(), ASSIGN_STAT | LOCAL_STAT))).map(|s| s.text_range());
        Some((code, def))
    }

    /// `a.b.c` of a path, a local root by its name.
    fn path_text(&self, p: &Path) -> String {
        let mut s = match &p.root {
            Root::Decl(d) => self.inf.names.decls[*d].name.clone(),
            Root::Global(g) => g.clone(),
        };
        for f in &p.fields {
            s.push('.');
            s.push_str(f);
        }
        s
    }

    /// The class an object is, for a field's name (`c.f`): LuaLS's
    /// `viewClass`, not for a global object.
    fn class_view(&self, obj: &SyntaxNode) -> Option<String> {
        match non_nil(self.inf.expr_type(obj)) {
            T::Name(c) if self.inf.classes.contains_key(&c) => Some(c),
            _ => None,
        }
    }

    /// The table a method is looked up in: the object's own path, the
    /// metatable's `__index` of a `setmetatable({}, mt)` value, what a local
    /// function returns, `self`'s class table.
    fn method_target(&self, obj: &SyntaxNode) -> Option<Path> {
        self.method_target_in(obj, 0)
    }

    fn method_target_in(&self, obj: &SyntaxNode, depth: u32) -> Option<Path> {
        if depth > 8 {
            return None;
        }
        let inf = &self.inf;
        let p = self.path_of(obj)?;
        let Root::Decl(d) = p.root else { return Some(p) };
        if !p.fields.is_empty() {
            return Some(p);
        }
        let decl = &inf.names.decls[d];
        // `self` of `function mt:f()`.
        if decl.kind == DeclKind::SelfParam {
            let stat = inf.root.covering_element(decl.range).ancestors().find(|a| a.kind() == FUNCTION_STAT)?;
            let fname = stat.children().find(|c| c.kind() == FUNC_NAME)?;
            let n = func_names(&fname).len();
            return self.func_name_path(&fname, n - 1);
        }
        let value = inf
            .decl_stat(d)
            .filter(|s| s.kind() == LOCAL_STAT)
            .and_then(|s| s.children().find(|c| c.kind() == EXPR_LIST))
            .and_then(|l| l.first_child());
        let Some(value) = value else { return Some(p) };
        self.value_table(&value, depth + 1).or(Some(p))
    }

    /// The table a value stands for: a name's own, a metatable's `__index`,
    /// what a local function returns.
    fn value_table(&self, value: &SyntaxNode, depth: u32) -> Option<Path> {
        if depth > 8 {
            return None;
        }
        match value.kind() {
            NAME_EXPR | INDEX_EXPR => self.method_target_in(value, depth + 1),
            CALL_EXPR => {
                let callee = value.first_child()?;
                let args: Vec<SyntaxNode> = value.children().find(|c| c.kind() == ARG_LIST)?.children().collect();
                if &self.inf.text[callee.text_range()] == "setmetatable" {
                    let mt = self.path_of(args.get(1)?)?;
                    let (occs, _) = self.scan(&mt.child("__index"));
                    let index = occs.iter().find_map(|o| o.value.clone())?;
                    return self.value_table(&index, depth + 1);
                }
                let body = self.inf.function_body(&callee)?;
                let ret = body.children().find(|c| c.kind() == BLOCK)?.children().find(|c| c.kind() == RETURN_STAT)?;
                let e = ret.children().find(|c| c.kind() == EXPR_LIST)?.first_child()?;
                self.value_table(&e, depth + 1)
            }
            _ => None,
        }
    }

    /// `function obj.f(a: any)` / `(method) mt:init(a: any)` with its return
    /// lines, for a `function ... end` statement.
    fn function_label(&self, stat: &SyntaxNode) -> String {
        let Some(body) = stat.children().find(|c| c.kind() == FUNC_BODY) else { return String::new() };
        let (sig, rets, is_async) = self.signature(&body);
        let fname = stat.children().find(|c| c.kind() == FUNC_NAME);
        let colon = fname.as_ref().is_some_and(|f| f.children_with_tokens().any(|e| e.kind() == COLON));
        let mut shown = fname.as_ref().map(|f| f.text().to_string().split_whitespace().collect::<String>()).unwrap_or_default();
        // `---@class c` on the local `t`: `c.f`.
        if let Some(f) = &fname
            && let [first, _] = func_names(f).as_slice()
            && let Some(Root::Decl(d)) = self.root_of(first)
            && let Some(c) = self.inf.decl_stat(d).and_then(|s| last_class_tag(self.inf.text, &s))
        {
            shown = format!("{c}{}", &shown[first.text().len()..]);
        }
        function_code(is_async, if colon { "(method)" } else { "function" }, &shown, &sig, &rets)
    }
}

/// ` {` and the field lines of a hover shape.
fn push_fields(code: &mut String, fields: Option<Vec<String>>) {
    let Some(fields) = fields else { return };
    code.push_str(" {");
    for f in fields {
        code.push_str("\n    ");
        code.push_str(&f);
        if !f.starts_with("...") {
            code.push(',');
        }
    }
    code.push_str("\n}");
}

fn function_code(is_async: bool, head: &str, shown: &str, sig: &str, rets: &[String]) -> String {
    let mut code = format!("{}{head} {shown}{sig}", if is_async { "(async) " } else { "" });
    for r in rets {
        code.push('\n');
        code.push_str(r);
    }
    code
}

fn func_names(fname: &SyntaxNode) -> Vec<SyntaxToken> {
    fname.children_with_tokens().filter_map(|x| x.into_token()).filter(|t| t.kind() == NAME).collect()
}

/// `t.x` / `t['x']`: the key.
fn index_key(e: &SyntaxNode) -> Option<String> {
    if let Some(n) = crate::lint::field_name(e) {
        return Some(n.text().to_string());
    }
    let k = e.children().nth(1).filter(|k| k.kind() == LITERAL_EXPR)?;
    let t = k.first_token().filter(|t| t.kind() == STRING)?;
    Some(crate::util::lua_string(t.text()))
}
