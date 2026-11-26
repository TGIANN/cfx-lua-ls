//! LuaCATS doc comments (`---@class`, `---@field`, `---@param`, ...): the tags of
//! a comment block and the type expressions in them, after LuaLS
//! `parser/luadoc.lua`. A type is parsed from the tokens of its line; what
//! follows it is the description. Nesting is capped, so a hostile comment
//! (`fun(): fun(): ...` thousands deep) cannot overflow the stack.

use crate::{TextRange, TextSize};

/// A type expression as written.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DocType {
    /// `integer`, `string`, `A.B`, `T` (a generic, resolved later).
    Name(String),
    /// `table<K, V>`, `list<string>`.
    Generic(String, Vec<DocType>),
    /// `T[]`.
    Array(Box<DocType>),
    /// `A | B`, at least two members.
    Union(Vec<DocType>),
    /// `T?`.
    Optional(Box<DocType>),
    Fun(Box<FunType>),
    /// `{ a: T, [K]: V }`.
    Table(Vec<TableField>),
    /// `[A, B]`.
    Tuple(Vec<DocType>),
    Str(String, Quote),
    Int(i64),
    Bool(bool),
    /// `` `T` ``: a generic captured from a string argument.
    Code(String),
    /// `...` (in a name position: unknown values).
    Dots,
}

/// The quote a string type was written with (`"b"` shows with it). It takes
/// no part in comparing types.
#[derive(Debug, Clone, Copy, Default)]
pub struct Quote(pub bool);

impl PartialEq for Quote {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for Quote {}

impl std::hash::Hash for Quote {
    fn hash<H: std::hash::Hasher>(&self, _: &mut H) {}
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FunType {
    pub params: Vec<FunParam>,
    pub rets: Vec<FunRet>,
    pub is_async: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FunParam {
    /// `...` for a vararg.
    pub name: String,
    pub ty: Option<DocType>,
    pub optional: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FunRet {
    pub ty: DocType,
    pub name: Option<String>,
    pub optional: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Key {
    Name(String),
    /// `[string]`, `[integer]`, `["a"]`, `[1]`.
    Index(DocType),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TableField {
    pub key: Key,
    pub ty: DocType,
    pub optional: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Visibility {
    Public,
    Protected,
    Private,
    Package,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tag {
    Class { name: String, generics: Vec<String>, parents: Vec<DocType>, exact: bool, partial: bool },
    Field { visibility: Option<Visibility>, key: Key, optional: bool, ty: DocType, desc: String },
    Param { name: String, optional: bool, ty: Option<DocType>, desc: String },
    Return { rets: Vec<FunRet>, desc: String },
    Type { tys: Vec<DocType> },
    Alias { name: String, generics: Vec<String>, ty: Option<DocType> },
    /// `---| 'a' # desc`: one more member of the alias above.
    AliasValue { ty: DocType, desc: String },
    Enum { name: String, key: bool },
    Generic { names: Vec<(String, Option<DocType>)> },
    Overload(DocType),
    Vararg(DocType),
    /// `---@cast x T`, `---@cast x +T`, `---@cast x -?`.
    Cast { name: String, ops: Vec<(char, DocType)> },
    Operator { op: String, param: Option<DocType>, ret: Option<DocType> },
    Deprecated(String),
    Nodiscard,
    Async,
    Meta,
    Visibility(Visibility),
    Module(String),
    Diagnostic(String),
    /// Any other tag, by name (`see`, `version`, `source`, unknown tags).
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocTag {
    pub tag: Tag,
    /// The whole line.
    pub range: TextRange,
    /// The name the tag declares (`---@param x`, `---@class A`, `---@field f`).
    pub name: Option<TextRange>,
    /// Every type name written in the tag (`A` in `---@param x A|B[]`, a
    /// class's parents, a generic's bound), in order.
    pub types: Vec<TextRange>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocError {
    pub range: TextRange,
    /// LuaLS's code: `luadoc-miss-type-name`, ...
    pub code: &'static str,
}

/// Type nesting accepted at most.
const MAX_DEPTH: usize = 32;

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Name(String),
    /// Content, and whether it was written with `"`.
    Str(String, bool),
    Int(i64),
    Code(String),
    Sym(&'static str),
}

struct Lexed {
    toks: Vec<(Tok, usize, usize)>,
}

fn name_start(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

fn name_cont(b: u8) -> bool {
    name_start(b) || matches!(b, b'.' | b'*' | b'-')
}

/// The tokens of `s` with byte offsets into it.
fn lex(s: &str) -> Lexed {
    let b = s.as_bytes();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        // Integer: `-`? digits, not followed by `.` or a name character.
        if c.is_ascii_digit() || (c == b'-' && b.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            let mut j = i + 1;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            let ends_number = j >= b.len() || !(b[j] == b'.' || name_cont(b[j]));
            if ends_number && let Ok(v) = s[i..j].parse::<i64>() {
                toks.push((Tok::Int(v), start, j));
                i = j;
                continue;
            }
        }
        if name_start(c) {
            let mut j = i + 1;
            while j < b.len() && name_cont(b[j]) {
                // `a..b` is not a name; a trailing `-` before `-` neither.
                if b[j] == b'.' && b.get(j + 1) == Some(&b'.') {
                    break;
                }
                j += 1;
            }
            // Names are ASCII-safe at the cut: name bytes >= 0x80 are whole UTF-8.
            toks.push((Tok::Name(s[i..j].to_string()), start, j));
            i = j;
            continue;
        }
        match c {
            b'\'' | b'"' => {
                let mut j = i + 1;
                let mut out = String::new();
                while j < b.len() && b[j] != c {
                    if b[j] == b'\\' && j + 1 < b.len() {
                        j += 1;
                    }
                    let ch_len = utf8_len(b[j]);
                    out.push_str(s.get(j..j + ch_len).unwrap_or(""));
                    j += ch_len;
                }
                let end = (j + 1).min(b.len());
                toks.push((Tok::Str(out, c == b'"'), start, end));
                i = end;
            }
            b'`' => {
                let close = s[i + 1..].find('`').map(|k| i + 1 + k);
                let end = close.unwrap_or(b.len());
                toks.push((Tok::Code(s[i + 1..end].to_string()), start, (end + 1).min(b.len())));
                i = (end + 1).min(b.len());
            }
            b'.' if s[i..].starts_with("...") => {
                toks.push((Tok::Sym("..."), start, i + 3));
                i += 3;
            }
            b'[' if s[i..].starts_with("[]") => {
                toks.push((Tok::Sym("[]"), start, i + 2));
                i += 2;
            }
            _ => {
                let sym = match c {
                    b':' => ":",
                    b'|' => "|",
                    b',' => ",",
                    b';' => ";",
                    b'<' => "<",
                    b'>' => ">",
                    b'(' => "(",
                    b')' => ")",
                    b'?' => "?",
                    b'+' => "+",
                    b'#' => "#",
                    b'{' => "{",
                    b'}' => "}",
                    b'*' => "*",
                    b'[' => "[",
                    b']' => "]",
                    b'-' => "-",
                    b'.' => ".",
                    b'@' => "@",
                    _ => "?other",
                };
                let len = utf8_len(c);
                toks.push((Tok::Sym(sym), start, i + len));
                i += len;
            }
        }
    }
    Lexed { toks }
}

fn utf8_len(first: u8) -> usize {
    match first {
        0xF0..=0xFF => 4,
        0xE0..=0xEF => 3,
        0xC0..=0xDF => 2,
        _ => 1,
    }
}

struct P<'a> {
    toks: &'a [(Tok, usize, usize)],
    i: usize,
    depth: usize,
    /// The whole line's text (for the description).
    line: &'a str,
    /// The tokens read as type names.
    types: Vec<usize>,
}

impl<'a> P<'a> {
    fn peek(&self) -> Option<&'a Tok> {
        self.toks.get(self.i).map(|t| &t.0)
    }

    fn peek_at(&self, k: usize) -> Option<&'a Tok> {
        self.toks.get(self.i + k).map(|t| &t.0)
    }

    fn sym(&self, s: &str) -> bool {
        matches!(self.peek(), Some(Tok::Sym(x)) if *x == s)
    }

    fn eat(&mut self, s: &str) -> bool {
        if self.sym(s) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn name(&mut self) -> Option<String> {
        match self.peek() {
            Some(Tok::Name(n)) => {
                self.i += 1;
                Some(n.clone())
            }
            _ => None,
        }
    }

    /// Byte offset of the next token in the line, or its end.
    fn pos(&self) -> usize {
        self.toks.get(self.i).map_or(self.line.len(), |t| t.1)
    }

    /// The text after the parsed part: the description, `#` / `@` dropped.
    fn rest(&self) -> String {
        let r = self.line[self.pos().min(self.line.len())..].trim();
        r.trim_start_matches(['#', '@']).trim().to_string()
    }

    /// `A | B | C?`.
    fn ty(&mut self) -> Option<DocType> {
        if self.depth >= MAX_DEPTH {
            return None;
        }
        self.depth += 1;
        let mut parts = Vec::new();
        // A leading `|` (multi-line alias style) is allowed.
        self.eat("|");
        while let Some(u) = self.unit() {
            parts.push(u);
            if !self.eat("|") {
                break;
            }
        }
        self.depth -= 1;
        let t = match parts.len() {
            0 => return None,
            1 => parts.pop()?,
            _ => DocType::Union(parts),
        };
        if self.eat("?") {
            return Some(DocType::Optional(Box::new(t)));
        }
        Some(t)
    }

    fn unit(&mut self) -> Option<DocType> {
        if self.depth >= MAX_DEPTH {
            return None;
        }
        self.depth += 1;
        let r = self.unit_inner();
        self.depth -= 1;
        r
    }

    fn unit_inner(&mut self) -> Option<DocType> {
        let mut t = match self.peek()? {
            Tok::Name(n) if n == "fun" && matches!(self.peek_at(1), Some(Tok::Sym("(" | "<"))) => {
                self.i += 1;
                self.fun(false)?
            }
            Tok::Name(n) if n == "async" && matches!(self.peek_at(1), Some(Tok::Name(f)) if f == "fun") => {
                self.i += 2;
                self.fun(true)?
            }
            Tok::Name(n) if n == "true" || n == "false" => {
                let v = n == "true";
                self.i += 1;
                DocType::Bool(v)
            }
            // `namespace.`T``: a generic captured with a prefix, kept as
            // `namespace.`T` in one `Code`.
            Tok::Name(n)
                if n.ends_with('.')
                    && let Some(Tok::Code(c)) = self.peek_at(1)
                    && self.toks.get(self.i + 1).map(|t| t.1) == self.toks.get(self.i).map(|t| t.2) =>
            {
                let code = format!("{n}`{c}");
                self.i += 2;
                DocType::Code(code)
            }
            Tok::Name(n) => {
                let n = n.clone();
                self.types.push(self.i);
                self.i += 1;
                DocType::Name(n)
            }
            Tok::Str(s, double) => {
                let (s, double) = (s.clone(), *double);
                self.i += 1;
                DocType::Str(s, Quote(double))
            }
            Tok::Int(v) => {
                let v = *v;
                self.i += 1;
                DocType::Int(v)
            }
            Tok::Code(c) => {
                let c = c.clone();
                self.i += 1;
                DocType::Code(c)
            }
            Tok::Sym("...") => {
                self.i += 1;
                DocType::Dots
            }
            Tok::Sym("{") => {
                self.i += 1;
                self.table()?
            }
            Tok::Sym("[") => {
                self.i += 1;
                let mut items = Vec::new();
                while !self.eat("]") {
                    items.push(self.ty()?);
                    if !self.eat(",") {
                        self.eat("]");
                        break;
                    }
                }
                DocType::Tuple(items)
            }
            Tok::Sym("(") => {
                self.i += 1;
                let inner = self.ty()?;
                self.eat(")");
                inner
            }
            _ => return None,
        };
        // `Name<A, B>`.
        if let DocType::Name(n) = &t
            && self.sym("<")
        {
            let n = n.clone();
            self.i += 1;
            let mut args = Vec::new();
            while let Some(a) = self.ty() {
                args.push(a);
                if !self.eat(",") {
                    break;
                }
            }
            self.eat(">");
            t = DocType::Generic(n, args);
        }
        while self.eat("[]") {
            t = DocType::Array(Box::new(t));
        }
        Some(t)
    }

    /// After `fun`: `(a: T, b?: U, ...: V): R1, R2`.
    fn fun(&mut self, is_async: bool) -> Option<DocType> {
        // `fun<T>(...)`: its generics become open ones (`<T>`), bound per call.
        let mut gens = Vec::new();
        if self.eat("<") {
            while let Some(g) = self.name() {
                gens.push(g);
                if !self.eat(",") {
                    break;
                }
            }
            self.eat(">");
        }
        if !self.eat("(") {
            return None;
        }
        let mut params = Vec::new();
        loop {
            if self.eat(")") {
                break;
            }
            let name = if self.eat("...") { "...".to_string() } else { self.name()? };
            let optional = self.eat("?");
            // `...: T` and the shorthand `...T`.
            let ty = if self.eat(":") || (name == "..." && !self.sym(",") && !self.sym(")")) { self.ty() } else { None };
            params.push(FunParam { name, ty, optional });
            if !self.eat(",") {
                self.eat(")");
                break;
            }
        }
        let mut rets = Vec::new();
        if self.eat(":") {
            let paren = self.eat("(");
            loop {
                // `name: T` returns, and `...: T` for the rest.
                let mut name = None;
                if let (Some(Tok::Name(n)), Some(Tok::Sym(":"))) = (self.peek(), self.peek_at(1)) {
                    name = Some(n.clone());
                    self.i += 2;
                } else if matches!(self.peek(), Some(Tok::Sym("..."))) && matches!(self.peek_at(1), Some(Tok::Sym(":"))) {
                    self.i += 2;
                    name = Some("...".to_string());
                }
                let Some(ty) = self.ty() else { break };
                let optional = matches!(ty, DocType::Optional(_));
                rets.push(FunRet { ty, name, optional });
                if !self.eat(",") {
                    break;
                }
            }
            if paren {
                self.eat(")");
            }
        }
        let mut f = FunType { params, rets, is_async };
        if !gens.is_empty() {
            for p in &mut f.params {
                if let Some(t) = &mut p.ty {
                    open_generic_names(t, &gens);
                }
            }
            for r in &mut f.rets {
                open_generic_names(&mut r.ty, &gens);
            }
        }
        Some(DocType::Fun(Box::new(f)))
    }

    /// After `{`: `a: T, [K]: V, b?: U }`.
    fn table(&mut self) -> Option<DocType> {
        let mut fields = Vec::new();
        loop {
            if self.eat("}") {
                break;
            }
            // `(f: fun(): number)`: a field in parentheses (its type may hold `,`).
            let paren = self.eat("(");
            let key = if self.eat("[") {
                let k = self.ty()?;
                self.eat("]");
                Key::Index(k)
            } else {
                Key::Name(self.name()?)
            };
            let optional = self.eat("?");
            if !self.eat(":") {
                return None;
            }
            let ty = self.ty()?;
            if paren && !self.eat(")") {
                return None;
            }
            fields.push(TableField { key, ty, optional });
            if !(self.eat(",") || self.eat(";")) {
                self.eat("}");
                break;
            }
        }
        Some(DocType::Table(fields))
    }
}

/// `T` -> `<T>` for each name of `gens` in `t`.
pub(crate) fn open_generic_names(t: &mut DocType, gens: &[String]) {
    match t {
        DocType::Name(n) if gens.contains(n) => *n = format!("<{n}>"),
        DocType::Generic(_, items) | DocType::Union(items) | DocType::Tuple(items) => items.iter_mut().for_each(|i| open_generic_names(i, gens)),
        DocType::Array(i) | DocType::Optional(i) => open_generic_names(i, gens),
        DocType::Table(fields) => fields.iter_mut().for_each(|f| open_generic_names(&mut f.ty, gens)),
        DocType::Fun(f) => {
            for p in &mut f.params {
                if let Some(t) = &mut p.ty {
                    open_generic_names(t, gens);
                }
            }
            for r in &mut f.rets {
                open_generic_names(&mut r.ty, gens);
            }
        }
        _ => {}
    }
}

/// One doc line's text after `---` (`@param x T desc`, or `| 'a'` for an alias
/// member). `at`: the line's offset in the file.
fn parse_line(text: &str, at: TextSize, errors: &mut Vec<DocError>) -> Option<DocTag> {
    let line_range = TextRange::at(at, TextSize::from(text.len() as u32));
    let trimmed = text.trim_start();
    let lead = text.len() - trimmed.len();
    if let Some(rest) = trimmed.strip_prefix('|') {
        let lexed = lex(rest);
        let mut p = P { toks: &lexed.toks, i: 0, depth: 0, line: rest, types: Vec::new() };
        // `---|>` (default) and `---|+` (additional) markers.
        p.eat(">");
        p.eat("+");
        let ty = p.unit()?;
        let rest_at = at + TextSize::from((lead + 1) as u32);
        let types = token_ranges(&p, rest_at);
        return Some(DocTag { tag: Tag::AliasValue { ty, desc: p.rest() }, range: line_range, name: None, types });
    }
    let rest = trimmed.strip_prefix('@')?;
    let tag_len = rest.bytes().take_while(|b| b.is_ascii_alphanumeric() || *b == b'_').count();
    let tag = &rest[..tag_len];
    let body = &rest[tag_len..];
    let body_at = at + TextSize::from((lead + 1 + tag_len) as u32);
    let lexed = lex(body);
    let mut p = P { toks: &lexed.toks, i: 0, depth: 0, line: body, types: Vec::new() };
    let span = |p: &P, i: usize| -> Option<TextRange> {
        let (_, s, e) = p.toks.get(i)?;
        Some(TextRange::new(body_at + TextSize::from(*s as u32), body_at + TextSize::from(*e as u32)))
    };
    let miss = |p: &P, code: &'static str, errors: &mut Vec<DocError>| {
        let o = body_at + TextSize::from(p.pos() as u32);
        errors.push(DocError { range: TextRange::empty(o), code });
    };
    let mut name_range = None;
    let tag = match tag {
        "class" => {
            let (mut exact, mut partial) = (false, false);
            if p.eat("(") {
                while let Some(a) = p.name() {
                    exact |= a == "exact";
                    partial |= a == "partial";
                    if !p.eat(",") {
                        break;
                    }
                }
                p.eat(")");
            }
            name_range = span(&p, p.i);
            let Some(name) = p.name() else {
                miss(&p, "luadoc-miss-class-name", errors);
                return None;
            };
            let mut generics = Vec::new();
            if p.eat("<") {
                while let Some(g) = p.name() {
                    generics.push(g);
                    if !p.eat(",") {
                        break;
                    }
                }
                p.eat(">");
            }
            let mut parents = Vec::new();
            if p.eat(":") {
                loop {
                    match p.unit() {
                        Some(t) => parents.push(t),
                        None => {
                            miss(&p, "luadoc-miss-class-extends-name", errors);
                            break;
                        }
                    }
                    if !p.eat(",") {
                        break;
                    }
                }
            }
            Tag::Class { name, generics, parents, exact, partial }
        }
        "field" => {
            let mut visibility = None;
            if let Some(Tok::Name(v)) = p.peek() {
                let vis = match v.as_str() {
                    "public" => Some(Visibility::Public),
                    "protected" => Some(Visibility::Protected),
                    "private" => Some(Visibility::Private),
                    "package" => Some(Visibility::Package),
                    _ => None,
                };
                // `---@field private T` names a field `private`.
                if vis.is_some() && matches!(p.peek_at(1), Some(Tok::Name(_) | Tok::Sym("["))) && p.peek_at(2).is_some() {
                    visibility = vis;
                    p.i += 1;
                }
            }
            name_range = span(&p, p.i);
            let key = if p.eat("[") {
                let Some(k) = p.ty() else {
                    miss(&p, "luadoc-miss-field-name", errors);
                    return None;
                };
                p.eat("]");
                Key::Index(k)
            } else {
                match p.name() {
                    Some(n) => Key::Name(n),
                    None => {
                        miss(&p, "luadoc-miss-field-name", errors);
                        return None;
                    }
                }
            };
            let optional = p.eat("?");
            let Some(ty) = p.ty() else {
                miss(&p, "luadoc-miss-field-extends", errors);
                return None;
            };
            Tag::Field { visibility, key, optional, ty, desc: p.rest() }
        }
        "param" => {
            name_range = span(&p, p.i);
            let name = if p.eat("...") {
                "...".to_string()
            } else {
                match p.name() {
                    Some(n) => n,
                    None => {
                        miss(&p, "luadoc-miss-param-name", errors);
                        return None;
                    }
                }
            };
            let optional = p.eat("?");
            let ty = p.ty();
            if ty.is_none() {
                miss(&p, "luadoc-miss-param-extends", errors);
            }
            Tag::Param { name, optional, ty, desc: p.rest() }
        }
        "return" => {
            let mut rets = Vec::new();
            loop {
                // `---@return ...T`: a vararg return of `T`; a bare `...` is a type.
                let dots = p.sym("...") && matches!(p.peek_at(1), Some(Tok::Name(_)));
                if dots {
                    p.i += 1;
                }
                let Some(mut ty) = p.ty() else { break };
                let mut optional = matches!(ty, DocType::Optional(_));
                if p.eat("?") {
                    ty = DocType::Optional(Box::new(ty));
                    optional = true;
                }
                let name = if dots {
                    Some("...".to_string())
                } else if p.sym("...") {
                    p.i += 1;
                    Some("...".to_string())
                } else {
                    p.name()
                };
                // `string errorData?`: the mark after the name.
                if name.is_some() && !optional && p.eat("?") {
                    ty = DocType::Optional(Box::new(ty));
                    optional = true;
                }
                rets.push(FunRet { ty, name, optional });
                if !p.eat(",") {
                    break;
                }
            }
            if rets.is_empty() {
                miss(&p, "luadoc-miss-type-name", errors);
                return None;
            }
            Tag::Return { rets, desc: p.rest() }
        }
        "type" => {
            let mut tys = Vec::new();
            loop {
                match p.ty() {
                    Some(t) => tys.push(t),
                    None => {
                        miss(&p, "luadoc-miss-type-name", errors);
                        break;
                    }
                }
                if !p.eat(",") {
                    break;
                }
            }
            if tys.is_empty() {
                return None;
            }
            Tag::Type { tys }
        }
        "alias" => {
            // `---@alias (partial) A boolean`: its attributes.
            if p.eat("(") {
                while p.name().is_some() {
                    if !p.eat(",") {
                        break;
                    }
                }
                p.eat(")");
            }
            name_range = span(&p, p.i);
            let Some(name) = p.name() else {
                miss(&p, "luadoc-miss-alias-name", errors);
                return None;
            };
            let mut generics = Vec::new();
            if p.eat("<") {
                while let Some(g) = p.name() {
                    generics.push(g);
                    if !p.eat(",") {
                        break;
                    }
                }
                p.eat(">");
            }
            // The members may follow on `---|` lines.
            let ty = p.ty();
            Tag::Alias { name, generics, ty }
        }
        "enum" => {
            let mut key = false;
            if p.eat("(") {
                while let Some(a) = p.name() {
                    key |= a == "key";
                    if !p.eat(",") {
                        break;
                    }
                }
                p.eat(")");
            }
            name_range = span(&p, p.i);
            let Some(name) = p.name() else {
                miss(&p, "luadoc-miss-alias-name", errors);
                return None;
            };
            Tag::Enum { name, key }
        }
        "generic" => {
            let mut names = Vec::new();
            loop {
                let Some(n) = p.name() else {
                    miss(&p, "luadoc-miss-generic-name", errors);
                    break;
                };
                let bound = if p.eat(":") { p.ty() } else { None };
                names.push((n, bound));
                if !p.eat(",") {
                    break;
                }
            }
            Tag::Generic { names }
        }
        "overload" => match p.ty() {
            Some(t @ DocType::Fun(_)) => Tag::Overload(t),
            _ => {
                miss(&p, "luadoc-miss-fun-after-overload", errors);
                return None;
            }
        },
        "vararg" => match p.ty() {
            Some(t) => Tag::Vararg(t),
            None => {
                miss(&p, "luadoc-miss-vararg-type", errors);
                return None;
            }
        },
        "cast" => {
            name_range = span(&p, p.i);
            let Some(name) = p.name() else {
                miss(&p, "luadoc-miss-local-name", errors);
                return None;
            };
            let mut ops = Vec::new();
            loop {
                let op = if p.eat("+") {
                    '+'
                } else if p.eat("-") {
                    '-'
                } else {
                    '='
                };
                // `+?` makes it optional (`T?`); `-?` takes nil away.
                if op == '+' && p.eat("?") {
                    ops.push(('?', DocType::Name("nil".into())));
                } else if op == '-' && p.eat("?") {
                    ops.push((op, DocType::Name("nil".into())));
                } else {
                    match p.ty() {
                        Some(t) => ops.push((op, t)),
                        None => break,
                    }
                }
                if !p.eat(",") {
                    break;
                }
            }
            Tag::Cast { name, ops }
        }
        "operator" => {
            let Some(op) = p.name() else {
                miss(&p, "luadoc-miss-operator-name", errors);
                return None;
            };
            let mut param = None;
            if p.eat("(") {
                param = p.ty();
                p.eat(")");
            }
            let ret = if p.eat(":") { p.ty() } else { None };
            Tag::Operator { op, param, ret }
        }
        "deprecated" => Tag::Deprecated(body.trim().to_string()),
        "nodiscard" => Tag::Nodiscard,
        "async" => Tag::Async,
        "meta" => Tag::Meta,
        "private" => Tag::Visibility(Visibility::Private),
        "protected" => Tag::Visibility(Visibility::Protected),
        "public" => Tag::Visibility(Visibility::Public),
        "package" => Tag::Visibility(Visibility::Package),
        "module" => match p.peek() {
            Some(Tok::Str(s, _)) => Tag::Module(s.clone()),
            _ => {
                miss(&p, "luadoc-miss-module-name", errors);
                return None;
            }
        },
        "diagnostic" => Tag::Diagnostic(body.trim().to_string()),
        other => Tag::Other(other.to_string()),
    };
    let types = token_ranges(&p, body_at);
    Some(DocTag { tag, range: line_range, name: name_range, types })
}

/// The ranges of the tokens `p` read as type names; `at`: where its text starts.
fn token_ranges(p: &P, at: TextSize) -> Vec<TextRange> {
    p.types
        .iter()
        .filter_map(|&i| p.toks.get(i))
        .map(|(_, s, e)| TextRange::new(at + TextSize::from(*s as u32), at + TextSize::from(*e as u32)))
        .collect()
}

/// The type names of an inline cast comment (`--[[@as A]]`, its text at `at`).
pub fn inline_as_types(comment: &str, at: TextSize) -> Vec<TextRange> {
    let Some(inner) = comment.strip_prefix("--[[").and_then(|c| c.strip_suffix("]]")) else { return Vec::new() };
    let lead = inner.len() - inner.trim_start().len();
    let Some(body) = inner.trim_start().strip_prefix("@as") else { return Vec::new() };
    let lexed = lex(body);
    let mut p = P { toks: &lexed.toks, i: 0, depth: 0, line: body, types: Vec::new() };
    p.ty();
    token_ranges(&p, at + TextSize::from((4 + lead + 3) as u32))
}

/// The tags of a run of doc lines: each `(offset of the text after "--", text)`.
/// Lines that are not tags are description and skipped.
pub fn parse_lines<'a>(lines: impl IntoIterator<Item = (TextSize, &'a str)>) -> (Vec<DocTag>, Vec<DocError>) {
    let mut tags = Vec::new();
    let mut errors = Vec::new();
    let lines: Vec<(TextSize, &str)> = lines.into_iter().collect();
    let mut i = 0;
    while i < lines.len() {
        let (at, line) = lines[i];
        i += 1;
        // `---@x` arrives as `-@x`: the third dash belongs to the doc marker.
        let Some(rest) = line.strip_prefix('-') else { continue };
        let at = at + TextSize::from(1);
        let t = rest.trim_start();
        if !(t.starts_with('@') || t.starts_with('|')) {
            continue;
        }
        // A `{` or `(` left open continues on the next plain `---` lines
        // (LuaLS `slideToNextLine`).
        let mut joined: Option<String> = None;
        // Where each joined line starts: (offset in the joined text, in the file).
        let mut segments = vec![(0usize, at)];
        let mut open = open_brackets(rest);
        while open > 0 && i < lines.len() {
            let Some(next) = lines[i].1.strip_prefix('-') else { break };
            let nt = next.trim_start();
            if nt.starts_with('@') || nt.starts_with('|') {
                break;
            }
            let j = joined.get_or_insert_with(|| rest.to_string());
            j.push(' ');
            segments.push((j.len(), lines[i].0 + TextSize::from(1)));
            j.push_str(next);
            open += open_brackets(next);
            i += 1;
        }
        let text = joined.as_deref().unwrap_or(rest);
        if let Some(mut tag) = parse_line(text, at, &mut errors) {
            if segments.len() > 1 {
                // The joined text's offsets back to the lines they came from.
                let map = |o: TextSize| {
                    let rel = usize::from(o - at);
                    let &(from, file) = segments.iter().rev().find(|(from, _)| *from <= rel).unwrap_or(&segments[0]);
                    file + TextSize::from((rel - from) as u32)
                };
                let range = |r: TextRange| TextRange::new(map(r.start()), map(r.end()).max(map(r.start())));
                tag.name = tag.name.map(range);
                tag.types = tag.types.iter().map(|r| range(*r)).collect();
            }
            tags.push(tag);
        }
    }
    (tags, errors)
}

/// `{` / `(` opened minus closed, outside quotes; stops at a `#` description.
fn open_brackets(s: &str) -> i32 {
    let mut n = 0;
    let mut quote = None;
    for c in s.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"' | '`') => quote = Some(c),
            (None, '{' | '(') => n += 1,
            (None, '}' | ')') => n -= 1,
            (None, '#') => break,
            _ => {}
        }
    }
    n
}

/// A run of consecutive `---` lines (LuaLS's bind group: an empty `---` line
/// keeps it going, a code or blank line ends it) and its tags.
pub struct DocGroup {
    pub tags: Vec<DocTag>,
    /// The end of the run's last line (before the line break).
    pub end: TextSize,
}

/// The file's doc groups, in order.
pub fn doc_groups(text: &str) -> Vec<DocGroup> {
    let mut groups = Vec::new();
    let mut run: Vec<(TextSize, &str)> = Vec::new();
    let mut end = TextSize::from(0);
    let mut at = 0usize;
    let mut flush = |run: &mut Vec<(TextSize, &str)>, end: TextSize| {
        if !run.is_empty() {
            groups.push(DocGroup { tags: parse_lines(std::mem::take(run)).0, end });
        }
    };
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        let t = body.trim_start();
        if t.starts_with("---") {
            let lead = body.len() - t.len();
            run.push((TextSize::from((at + lead + 2) as u32), &t[2..]));
            end = TextSize::from((at + body.len()) as u32);
        } else {
            flush(&mut run, end);
        }
        at += line.len();
    }
    flush(&mut run, end);
    groups
}

/// The tags of a doc block given as text (lines starting with `---`), with
/// `start` the block's offset in the file.
pub fn parse_block(text: &str, start: TextSize) -> (Vec<DocTag>, Vec<DocError>) {
    let mut lines = Vec::new();
    let mut at = 0usize;
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        let trimmed = body.trim_start();
        let lead = body.len() - trimmed.len();
        if let Some(after) = trimmed.strip_prefix("--") {
            lines.push((start + TextSize::from((at + lead + 2) as u32), after));
        }
        at += line.len();
    }
    parse_lines(lines)
}

/// `DocType` back to text, LuaLS style (`integer?`, `fun(a: T): R`,
/// `table<K, V>`, `{ a: T }`).
pub fn view(t: &DocType) -> String {
    let mut s = String::new();
    write_view(t, &mut s, 0);
    s
}

fn write_view(t: &DocType, s: &mut String, depth: usize) {
    if depth > MAX_DEPTH {
        s.push_str("...");
        return;
    }
    match t {
        DocType::Name(n) => s.push_str(n),
        DocType::Generic(n, args) => {
            s.push_str(n);
            s.push('<');
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    s.push_str(", ");
                }
                // An open generic shows bare inside `table<K, V>` (LuaLS).
                match a {
                    DocType::Name(g) if g.len() > 2 && g.starts_with('<') && g.ends_with('>') => s.push_str(&g[1..g.len() - 1]),
                    a => write_view(a, s, depth + 1),
                }
            }
            s.push('>');
        }
        DocType::Array(inner) => {
            let wrap = matches!(**inner, DocType::Union(_) | DocType::Optional(_) | DocType::Fun(_));
            if wrap {
                s.push('(');
            }
            write_view(inner, s, depth + 1);
            if wrap {
                s.push(')');
            }
            s.push_str("[]");
        }
        DocType::Union(parts) => {
            for (i, p) in parts.iter().enumerate() {
                if i > 0 {
                    s.push('|');
                }
                write_view(p, s, depth + 1);
            }
        }
        DocType::Optional(inner) => {
            let wrap = matches!(**inner, DocType::Union(_) | DocType::Fun(_));
            if wrap {
                s.push('(');
            }
            write_view(inner, s, depth + 1);
            if wrap {
                s.push(')');
            }
            s.push('?');
        }
        DocType::Fun(f) => {
            if f.is_async {
                s.push_str("async ");
            }
            s.push_str("fun(");
            for (i, p) in f.params.iter().enumerate() {
                if i > 0 {
                    s.push_str(", ");
                }
                s.push_str(&p.name);
                if p.optional {
                    s.push('?');
                }
                match &p.ty {
                    Some(t) if p.name == "..." => write_view(t, s, depth + 1),
                    Some(t) => {
                        s.push_str(": ");
                        write_view(t, s, depth + 1);
                    }
                    // LuaLS writes an untyped parameter `any`.
                    None if p.name == "..." => s.push_str("any"),
                    None => s.push_str(": any"),
                }
            }
            s.push(')');
            if !f.rets.is_empty() {
                s.push(':');
                // Named returns go in parentheses: `fun():(x: number)`.
                let named = f.rets.iter().any(|r| r.name.as_ref().is_some_and(|n| n != "..."));
                if named {
                    s.push('(');
                }
                for (i, r) in f.rets.iter().enumerate() {
                    if i > 0 {
                        s.push_str(", ");
                    }
                    if r.name.as_deref() == Some("...") {
                        s.push_str("...");
                    } else if let Some(n) = r.name.as_ref().filter(|_| named) {
                        s.push_str(n);
                        s.push_str(": ");
                    }
                    if r.ty == DocType::Dots {
                        s.push_str("...unknown");
                    } else {
                        write_view(&r.ty, s, depth + 1);
                    }
                }
                if named {
                    s.push(')');
                }
            }
        }
        DocType::Table(fields) => {
            s.push_str("{ ");
            for (i, f) in fields.iter().enumerate() {
                if i > 0 {
                    s.push_str(", ");
                }
                match &f.key {
                    Key::Name(n) => s.push_str(n),
                    Key::Index(k) => {
                        s.push('[');
                        write_view(k, s, depth + 1);
                        s.push(']');
                    }
                }
                if f.optional {
                    s.push('?');
                }
                s.push_str(": ");
                write_view(&f.ty, s, depth + 1);
            }
            s.push_str(" }");
        }
        DocType::Tuple(items) => {
            s.push('[');
            for (i, it) in items.iter().enumerate() {
                if i > 0 {
                    s.push_str(", ");
                }
                write_view(it, s, depth + 1);
            }
            s.push(']');
        }
        // `"'a'"`, a quoted literal in a string, shows as that literal.
        DocType::Str(v, _) if v.len() >= 2 && (v.starts_with('\'') && v.ends_with('\'') || v.starts_with('"') && v.ends_with('"')) => s.push_str(v),
        DocType::Str(v, q) => {
            let c = if q.0 { '"' } else { '\'' };
            s.push(c);
            s.push_str(v);
            s.push(c);
        }
        DocType::Int(v) => s.push_str(&v.to_string()),
        DocType::Bool(v) => s.push_str(if *v { "true" } else { "false" }),
        DocType::Code(c) => {
            let (prefix, g) = c.rsplit_once('`').unwrap_or(("", c));
            s.push_str(prefix);
            s.push('`');
            s.push_str(g);
            s.push('`');
        }
        DocType::Dots => s.push_str("..."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(text: &str) -> Vec<Tag> {
        parse_block(text, TextSize::from(0)).0.into_iter().map(|t| t.tag).collect()
    }

    fn ty(s: &str) -> String {
        match &tags(&format!("---@type {s}"))[0] {
            Tag::Type { tys } => view(&tys[0]),
            t => panic!("{t:?}"),
        }
    }

    #[test]
    fn types() {
        for s in [
            "integer",
            "string?",
            "A|B",
            "integer[]",
            "string[][]",
            "table<string, integer>",
            "fun(a: string, b?: integer, ...any):boolean, string",
            "fun()",
            "{ name: string, [integer]: boolean, age?: integer }",
            "'a'|'b'",
            "1",
            "true",
            "`T`",
            "(A|B)[]",
            "async fun(cb: fun(x: integer))",
            "Wrapper<string>",
        ] {
            assert_eq!(ty(s), s, "{s}");
        }
        assert_eq!(ty("A | B"), "A|B");
        assert_eq!(ty("vector2 | vector3 | vector4"), "vector2|vector3|vector4");
    }

    #[test]
    fn esx_and_ox_lines() {
        let t = tags(
            "---@class xPlayer:StaticPlayer\n---@field getAccounts fun(minimal?: boolean): ESXAccount[]|table<string,number>  # Get all accounts\n---@field hasItem fun(item: string): ESXInventoryItem|false, number? # Check\n---@field coords vector4|{x: number, y: number, z: number, heading: number}\n---@field n 2\n",
        );
        let Tag::Class { name, parents, .. } = &t[0] else { panic!() };
        assert_eq!((name.as_str(), view(&parents[0]).as_str()), ("xPlayer", "StaticPlayer"));
        let Tag::Field { key, ty, desc, .. } = &t[1] else { panic!() };
        assert_eq!(key, &Key::Name("getAccounts".into()));
        assert_eq!(view(ty), "fun(minimal?: boolean):ESXAccount[]|table<string, number>");
        assert_eq!(desc, "Get all accounts");
        let Tag::Field { ty, .. } = &t[2] else { panic!() };
        assert_eq!(view(ty), "fun(item: string):ESXInventoryItem|false, number?");
        let Tag::Field { ty, .. } = &t[3] else { panic!() };
        assert_eq!(view(ty), "vector4|{ x: number, y: number, z: number, heading: number }");
        let Tag::Field { ty, .. } = &t[4] else { panic!() };
        assert_eq!(ty, &DocType::Int(2));
    }

    #[test]
    fn tags_and_descriptions() {
        let t = tags(
            "--- Adds money.\n---@param src number the player\n---@param amount? integer\n---@param ... any\n---@return boolean ok, string? err\n---@deprecated use Pay\n---@nodiscard\n",
        );
        assert_eq!(t[0], Tag::Param { name: "src".into(), optional: false, ty: Some(DocType::Name("number".into())), desc: "the player".into() });
        assert!(matches!(&t[1], Tag::Param { optional: true, .. }));
        assert!(matches!(&t[2], Tag::Param { name, .. } if name == "..."));
        let Tag::Return { rets, .. } = &t[3] else { panic!() };
        assert_eq!(rets.len(), 2);
        assert_eq!((rets[0].name.as_deref(), rets[1].name.as_deref(), rets[1].optional), (Some("ok"), Some("err"), true));
        assert_eq!(t[4], Tag::Deprecated("use Pay".into()));
        assert_eq!(t[5], Tag::Nodiscard);
        let t = tags("---@alias Side\n---| 'client' # the client\n---| 'server'\n");
        assert!(matches!(&t[0], Tag::Alias { name, ty: None, .. } if name == "Side"));
        assert_eq!(t[1], Tag::AliasValue { ty: DocType::Str("client".into(), Quote::default()), desc: "the client".into() });
        let t = tags("---@field private secret string\n---@field private string\n---@generic T: table, K\n---@cast x +string\n---@operator add(vector3|number): vector3\n");
        assert!(matches!(&t[0], Tag::Field { visibility: Some(Visibility::Private), key: Key::Name(n), .. } if n == "secret"));
        assert!(matches!(&t[1], Tag::Field { visibility: None, key: Key::Name(n), .. } if n == "private"));
        assert!(matches!(&t[2], Tag::Generic { names } if names.len() == 2));
        assert!(matches!(&t[3], Tag::Cast { name, ops } if name == "x" && ops[0].0 == '+'));
        assert!(matches!(&t[4], Tag::Operator { op, .. } if op == "add"));
    }

    #[test]
    fn errors_and_deep_input() {
        let t = tags("---@param list {\n---  [1]: string,\n---  n?: integer,\n--- }\n---@return integer status, string errorData?\n---@return ...\n");
        let Tag::Param { ty: Some(ty), .. } = &t[0] else { panic!("{t:?}") };
        assert_eq!(view(ty), "{ [1]: string, n?: integer }");
        let Tag::Return { rets, .. } = &t[1] else { panic!() };
        assert_eq!((view(&rets[1].ty).as_str(), rets[1].name.as_deref()), ("string?", Some("errorData")));
        let Tag::Return { rets, .. } = &t[2] else { panic!() };
        assert_eq!(rets[0].ty, DocType::Dots);
        let (t, e) = parse_block("---@param\n---@class\n---@type\n", TextSize::from(0));
        assert!(t.is_empty());
        assert_eq!(e.iter().map(|e| e.code).collect::<Vec<_>>(), ["luadoc-miss-param-name", "luadoc-miss-class-name", "luadoc-miss-type-name"]);
        let deep = format!("---@type {}integer{}\n", "fun(): ".repeat(5_000), "");
        let _ = parse_block(&deep, TextSize::from(0));
        let deep = format!("---@type {}integer{}\n", "table<".repeat(5_000), ">".repeat(5_000));
        let _ = parse_block(&deep, TextSize::from(0));
        let deep = format!("---@type {}x{}\n", "(".repeat(5_000), ")".repeat(5_000));
        let _ = parse_block(&deep, TextSize::from(0));
    }
}
