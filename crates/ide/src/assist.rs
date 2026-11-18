//! Hover, completion and signature help from what is known without type
//! inference: the embedded library (natives, the CFX runtime, Lua 5.4), the
//! locals in scope, the names the file itself uses, and what the workspace's
//! other files and resources define (framework objects, ox_lib's `lib`,
//! exports), read from their own source.

use crate::api::{self, Ref};
use crate::library::{Game, LibItem, Origin, Side, library};
use crate::workspace::{ApiItem, Workspace};
use std::path::Path;
use crate::manifest::FileSide;
use crate::names::{Attrib, DeclKind, Names, Symbol, Target};
use crate::selection::token_at;
use crate::{Parse, TextRange, TextSize};
use cfx_lua_syntax::SyntaxKind::*;
use cfx_lua_syntax::{SyntaxNode, SyntaxToken};
use std::collections::HashSet;

pub struct Hover {
    pub range: TextRange,
    pub markdown: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionKind {
    Keyword,
    Local,
    Function,
    Global,
    Field,
    /// A class, alias or enum name in a doc comment.
    Class,
    /// A `---@diagnostic` mode.
    Enum,
    /// A literal value a type allows (`"a"` of `"a"|"b"`).
    EnumMember,
    /// A field of the expected type, in a table constructor.
    Property,
    /// A parameter name after `---@param`.
    Interface,
    /// A doc tag after `---@`.
    Event,
    /// Several names or lines at once.
    Snippet,
    /// A diagnostic code.
    Value,
    /// A `---@operator` name.
    Operator,
    /// A word of the file.
    Text,
}

#[derive(Debug, Clone)]
pub struct Completion {
    pub label: String,
    pub kind: CompletionKind,
    pub detail: Option<String>,
    /// Marked `---@deprecated` where it is defined.
    pub deprecated: bool,
    /// The text the label replaces when it is not just the word before the
    /// cursor (`AAA.BBB` after `AAA.`).
    pub replace: Option<TextRange>,
    /// What to insert when it is not the label.
    pub insert: Option<String>,
    /// More changes made with it (`goto continue` adds `::continue::`).
    pub edits: Vec<(TextRange, String)>,
}

/// LuaLS's `completion.showWord`: the file's other words as plain text items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShowWord {
    Enable,
    /// Only when nothing else matched (LuaLS's default).
    Fallback,
    Disable,
}

/// How completion was asked for.
#[derive(Debug, Clone, Copy)]
pub struct CompleteOptions {
    /// The character the client typed to ask (`.`, `(`, a space), if any.
    pub trigger: Option<char>,
    pub show_word: ShowWord,
}

impl Default for CompleteOptions {
    fn default() -> Self {
        CompleteOptions { trigger: None, show_word: ShowWord::Fallback }
    }
}

/// What a feature knows about the file beyond its text.
#[derive(Clone, Copy)]
pub struct FileCtx<'a> {
    pub game: Game,
    pub side: FileSide,
    /// The workspace and the file's path, for what other files define.
    pub ws: Option<(&'a Workspace, &'a Path)>,
}

impl FileCtx<'_> {
    pub fn plain(game: Game, side: FileSide) -> FileCtx<'static> {
        FileCtx { game, side, ws: None }
    }
}

/// A field, method, global or export the workspace can describe.
pub(crate) enum ApiTarget {
    Member { root: Ref, fields: Vec<String> },
    Export { resource: String, name: String },
}

/// Fields followed at most from a root name.
const MAX_CHAIN: usize = 8;

/// `a.b['c']` ending at a name: the root as a `Ref` of the file and the fields.
pub(crate) fn chain(text: &str, names: &Names, e: &SyntaxNode) -> Option<(Ref, Vec<String>)> {
    let mut fields = Vec::new();
    let mut e = e.clone();
    while e.kind() == INDEX_EXPR {
        if fields.len() == MAX_CHAIN {
            return None;
        }
        let f = match crate::lint::field_name(&e) {
            Some(f) => f.text().to_string(),
            None => {
                let key = e.children().nth(1).filter(|k| k.kind() == LITERAL_EXPR)?;
                crate::util::unquote(key.first_token()?.text())?.to_string()
            }
        };
        fields.push(f);
        e = e.first_child()?;
    }
    if e.kind() != NAME_EXPR {
        return None;
    }
    let t = e.first_token()?;
    let i = names.refs.binary_search_by_key(&t.text_range().start(), |x| x.range.start()).ok()?;
    let root = match names.refs[i].target {
        Target::Global => Ref::Global(t.text().to_string()),
        Target::Decl(d) => Ref::Local(d as u32),
    };
    let _ = text;
    fields.reverse();
    Some((root, fields))
}

/// The workspace thing a name token names.
pub(crate) fn api_target(text: &str, names: &Names, tok: &SyntaxToken) -> Option<ApiTarget> {
    let parent = tok.parent()?;
    match parent.kind() {
        METHOD_CALL_EXPR if api::method_name(&parent).is_some_and(|m| m == *tok) => {
            let object = parent.first_child()?;
            if let Some(resource) = api::exports_resource(text, names, &object) {
                return Some(ApiTarget::Export { resource, name: tok.text().to_string() });
            }
            let (root, mut fields) = chain(text, names, &object)?;
            fields.push(tok.text().to_string());
            Some(ApiTarget::Member { root, fields })
        }
        INDEX_EXPR if crate::lint::field_name(&parent).is_some_and(|f| f == *tok) => {
            let (root, fields) = chain(text, names, &parent)?;
            Some(ApiTarget::Member { root, fields })
        }
        NAME_EXPR => {
            let (root, fields) = chain(text, names, &parent)?;
            matches!(root, Ref::Global(_)).then_some(ApiTarget::Member { root, fields })
        }
        _ => None,
    }
}

/// The item a target names, seen from the file.
pub(crate) fn api_lookup(ws: &Workspace, file: &Path, overlay: Option<&api::FileApi>, t: &ApiTarget) -> Option<ApiItem> {
    match t {
        ApiTarget::Member { root, fields } => ws.api_item(file, overlay, root, fields),
        ApiTarget::Export { resource, name } => ws.export_item(file, resource, name),
    }
}

/// `resource/rel/path.lua:line` for a definition.
fn location_label(ws: &Workspace, file: &Path, offset: u32) -> String {
    let line = std::fs::read(file)
        .map(|b| b.iter().take(offset as usize).filter(|&&c| c == b'\n').count() + 1)
        .unwrap_or(1);
    let shown = match ws.resource(file) {
        Some((root, name, _)) => format!("{name}/{}", file.strip_prefix(root).unwrap_or(file).to_string_lossy().replace('\\', "/")),
        None => file.to_string_lossy().into_owned(),
    };
    format!("{shown}:{line}")
}

fn render_api(ws: &Workspace, item: &ApiItem, prefix: &str) -> String {
    let code = match (&item.params, item.ty.as_deref()) {
        (Some(_), Some(t)) if t.starts_with("fun(") => format!("function {prefix}{}{}", item.name, &t[3..]),
        (Some(p), _) => format!("function {prefix}{}({})", item.name, p.join(", ")),
        (None, Some(t)) => format!("(field) {prefix}{}: {t}", item.name),
        (None, None) => format!("{prefix}{}", item.name),
    };
    let mut md = format!("```lua\n{code}\n```");
    let doc = render_doc(&item.doc);
    if let Some(d) = item.deprecated() {
        md.push_str(&format!("\n\n**Deprecated.** {d}"));
    }
    if !doc.is_empty() {
        md.push_str("\n\n");
        md.push_str(&doc);
    }
    if let Some((file, offset)) = &item.location {
        md.push_str(&format!("\n\n`{}`", location_label(ws, file, *offset)));
    }
    md
}

fn api_completion(item: &ApiItem) -> Completion {
    Completion {
        label: item.name.clone(),
        kind: if item.params.is_some() { CompletionKind::Function } else { CompletionKind::Field },
        detail: Some(match (&item.params, &item.ty) {
            (None, Some(t)) => t.clone(),
            (Some(_), Some(t)) if t.starts_with("fun(") => format!("{}{}", item.name, &t[3..]),
            _ => item.signature(),
        }),
        deprecated: item.deprecated().is_some(),
        replace: None, insert: None, edits: Vec::new(),
    }
}

/// LuaLS's kind for a global: Enum when the file first gives it a literal,
/// Function for a function, else Field.
fn global_kind(root: &SyntaxNode, text: &str, g: &str) -> CompletionKind {
    for st in root.descendants().filter(|s| matches!(s.kind(), ASSIGN_STAT | FUNCTION_STAT)) {
        if st.kind() == FUNCTION_STAT {
            if st.children().find(|c| c.kind() == FUNC_NAME).is_some_and(|n| n.text() == g) {
                return CompletionKind::Function;
            }
            continue;
        }
        let Some(vars) = st.children().find(|c| c.kind() == VAR_LIST) else { continue };
        let Some(i) = vars.children().position(|v| v.kind() == NAME_EXPR && &text[v.text_range()] == g) else { continue };
        let value = st.children().find(|c| c.kind() == EXPR_LIST).and_then(|l| l.children().nth(i));
        return match value.as_ref().map(|v| (v.kind(), v)) {
            Some((LITERAL_EXPR, v)) if v.first_token().is_some_and(|t| t.kind() != NIL_KW) => CompletionKind::Enum,
            Some((FUNCTION_EXPR, _)) => CompletionKind::Function,
            _ => CompletionKind::Global,
        };
    }
    // Only read here: a word.
    CompletionKind::Text
}

/// `continue` inside a loop: the keyword, and a snippet that writes `goto
/// continue` and adds `::continue::` before the loop's `end` when it has none.
fn continue_items(parse: &Parse, text: &str, offset: TextSize, start: usize) -> Vec<Completion> {
    let Some(tok) = token_at(&parse.syntax(), offset).left_biased() else { return Vec::new() };
    let Some(lp) = tok.parent_ancestors().find(|a| matches!(a.kind(), WHILE_STAT | NUMERIC_FOR_STAT | GENERIC_FOR_STAT | REPEAT_STAT)) else { return Vec::new() };
    let close = if lp.kind() == REPEAT_STAT { UNTIL_KW } else { END_KW };
    let Some(end_kw) = lp.children_with_tokens().filter_map(|e| e.into_token()).find(|t| t.kind() == close) else { return Vec::new() };
    let mut edits = Vec::new();
    let before = text[..start].trim_end();
    if !before.ends_with("goto") {
        edits.push((TextRange::empty(TextSize::from(start as u32)), "goto ".to_string()));
    }
    let has_label = lp.descendants().any(|n| n.kind() == LABEL_STAT && n.text().to_string().contains("continue"));
    if !has_label {
        let at = end_kw.text_range().start();
        let line_start = text[..usize::from(at)].rfind('\n').map_or(0, |i| i + 1);
        let indent = &text[line_start..usize::from(at)];
        let label = if indent.chars().all(|c| c == ' ' || c == '\t') { format!("    ::continue::\n{indent}") } else { "::continue::".to_string() };
        edits.push((TextRange::empty(at), label));
    }
    vec![
        Completion { label: "continue".into(), kind: CompletionKind::Keyword, detail: None, deprecated: false, replace: None, insert: None, edits: Vec::new() },
        Completion { label: "goto continue ..".into(), kind: CompletionKind::Snippet, detail: None, deprecated: false, replace: None, insert: Some("continue".into()), edits },
    ]
}

/// The event-name string under the cursor (`TriggerServerEvent('a:b')`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventAt {
    pub name: String,
    /// Between the quotes (to the end of an unterminated string).
    pub content: TextRange,
    /// The call registers a handler (`RegisterNetEvent`, `AddEventHandler`).
    pub handler: bool,
}

const EVENT_HANDLERS: &[&str] = &["RegisterNetEvent", "AddEventHandler", "RegisterServerEvent"];
const EVENT_TRIGGERS: &[&str] =
    &["TriggerEvent", "TriggerServerEvent", "TriggerClientEvent", "TriggerLatentClientEvent", "TriggerLatentServerEvent"];

/// The event name being typed or pointed at: the first argument of an event
/// call, as a short string literal.
pub fn event_at(parse: &Parse, text: &str, offset: TextSize) -> Option<EventAt> {
    let root = parse.syntax();
    let at = token_at(&root, offset);
    let t = [at.clone().left_biased(), at.right_biased()]
        .into_iter()
        .flatten()
        .find(|t| t.kind() == STRING && t.text_range().start() < offset && offset <= t.text_range().end())?;
    let s = t.text();
    let quote = s.chars().next().filter(|q| matches!(q, '\'' | '"'))?;
    let r = t.text_range();
    let closed = s.len() >= 2 && s.ends_with(quote);
    let content = TextRange::new(r.start() + TextSize::from(1), if closed { r.end() - TextSize::from(1) } else { r.end() });
    if offset > content.end() {
        return None;
    }
    let lit = t.parent().filter(|p| p.kind() == LITERAL_EXPR)?;
    let args = lit.parent().filter(|p| p.kind() == ARG_LIST)?;
    if args.children().next()? != lit {
        return None;
    }
    let call = args.parent().filter(|c| c.kind() == CALL_EXPR)?;
    let callee = call.first_child().filter(|c| c.kind() == NAME_EXPR)?;
    let name = &text[callee.text_range()];
    let handler = EVENT_HANDLERS.contains(&name);
    if !handler && !EVENT_TRIGGERS.contains(&name) {
        return None;
    }
    Some(EventAt { name: text[content].to_string(), content, handler })
}

pub struct Completions {
    pub items: Vec<Completion>,
    /// More would match: the client should ask again as the user types.
    pub incomplete: bool,
}

pub struct Signature {
    /// `GetEntityCoords(entity, alive)`.
    pub label: String,
    pub params: Vec<(String, Option<String>)>,
    pub doc: String,
}

pub struct SignatureHelp {
    pub signatures: Vec<Signature>,
    pub active_param: usize,
}

const KEYWORDS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in", "local", "nil",
    "not", "or", "repeat", "return", "then", "true", "until", "while",
];

/// Most completion items sent at once.
const MAX_ITEMS: usize = 200;

fn is_global(names: &Names, r: TextRange) -> bool {
    names
        .refs
        .binary_search_by_key(&r.start(), |x| x.range.start())
        .is_ok_and(|i| names.refs[i].range == r && names.refs[i].target == Target::Global)
}

/// Library paths have at most this many parts (`Citizen.InvokeNative`).
const MAX_PATH_PARTS: usize = 4;

/// `a.b.c` for a short chain of fields that starts at a global.
pub(crate) fn expr_path(names: &Names, e: &SyntaxNode) -> Option<String> {
    let mut fields = Vec::new();
    let mut e = e.clone();
    while e.kind() == INDEX_EXPR {
        if fields.len() == MAX_PATH_PARTS {
            return None;
        }
        fields.push(crate::lint::field_name(&e)?.text().to_string());
        e = e.first_child()?;
    }
    if e.kind() != NAME_EXPR {
        return None;
    }
    let t = e.first_token()?;
    if !is_global(names, t.text_range()) {
        return None;
    }
    fields.push(t.text().to_string());
    fields.reverse();
    Some(fields.join("."))
}

/// The global path a name token ends: `GetEntityCoords`, `string.format`.
fn token_path(names: &Names, tok: &SyntaxToken) -> Option<String> {
    let parent = tok.parent()?;
    match parent.kind() {
        NAME_EXPR => is_global(names, tok.text_range()).then(|| tok.text().to_string()),
        INDEX_EXPR if crate::lint::field_name(&parent).is_some_and(|f| f == *tok) => {
            Some(format!("{}.{}", expr_path(names, &parent.first_child()?)?, tok.text()))
        }
        _ => None,
    }
}

pub(crate) fn items_for(path: &str, game: Game) -> Vec<&'static LibItem> {
    let all = library().get(path);
    let fit: Vec<&LibItem> = all.iter().filter(|i| i.game == Game::Any || i.game == game).collect();
    if fit.is_empty() { all.iter().collect() } else { fit }
}

/// Doc comment text for Markdown: `@param` / `@return` lines as a list, other
/// annotations dropped.
fn render_doc(doc: &str) -> String {
    let mut prose = Vec::new();
    let mut params = Vec::new();
    for line in doc.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("@param") {
            let mut parts = rest.trim().splitn(3, char::is_whitespace);
            let (name, ty, desc) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().unwrap_or(""));
            params.push(format!("- `{name}` `{ty}` {desc}").trim_end().to_string());
        } else if let Some(rest) = l.strip_prefix("@return") {
            params.push(format!("- returns `{}`", rest.trim()));
        } else if !l.starts_with('@') {
            prose.push(line.trim_end());
        }
    }
    let mut out = prose.join("\n").trim().to_string();
    if !params.is_empty() {
        out.push_str("\n\n");
        out.push_str(&params.join("\n"));
    }
    out
}

fn render_items(items: &[&LibItem]) -> String {
    let lib = library();
    items
        .iter()
        .take(3)
        .map(|i| format!("```lua\n{}\n```\n{}", lib.declaration(i), render_doc(&lib.doc(i))))
        .collect::<Vec<_>>()
        .join("\n\n---\n\n")
}

fn name_token(parse: &Parse, offset: TextSize) -> Option<SyntaxToken> {
    token_at(&parse.syntax(), offset).find(|t| t.kind() == NAME)
}

/// The comment lines a statement starts with (its node begins at the comments
/// above it).
fn leading_comments(text: &str, owner: TextRange) -> String {
    text[owner]
        .lines()
        .take_while(|l| l.trim_start().starts_with("--"))
        .map(|l| l.trim_start().trim_start_matches('-').trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The body of the function `range` holds (a `local function` statement or a
/// function expression).
fn function_body_in(parse: &Parse, range: TextRange) -> Option<SyntaxNode> {
    let root = parse.syntax();
    let node = match root.covering_element(range) {
        rowan::NodeOrToken::Node(n) => n,
        rowan::NodeOrToken::Token(t) => t.parent()?,
    };
    node.descendants().find(|n| n.kind() == FUNC_BODY)
}

/// The literal a local starts with (`1`, `-1000`, `'a'`, `true`), as written.
fn literal_value(parse: &Parse, text: &str, d: &crate::Decl) -> Option<String> {
    let stat = parse.syntax().covering_element(d.range).ancestors().find(|a| a.kind() == LOCAL_STAT)?;
    let index = stat.children().filter(|c| c.kind() == LOCAL_NAME).position(|n| n.text_range().contains_range(d.range))?;
    let value = stat.children().find(|c| c.kind() == EXPR_LIST)?.children().nth(index)?;
    let (literal, minus) = match value.kind() {
        LITERAL_EXPR => (value.clone(), false),
        UNARY_EXPR if text[value.text_range()].starts_with('-') => (value.children().find(|c| c.kind() == LITERAL_EXPR)?, true),
        _ => return None,
    };
    let lit = &text[literal.text_range()];
    if lit == "nil" || lit.starts_with('[') || (minus && !lit.starts_with(|c: char| c.is_ascii_digit())) {
        return None;
    }
    Some(if minus { format!("-{lit}") } else { lit.to_string() })
}

/// `(a, b)` of the function in `range`.
pub(crate) fn param_list(text: &str, range: TextRange) -> &str {
    let s = &text[range];
    match (s.find('('), s.find(')')) {
        (Some(a), Some(b)) if a < b => &s[a..=b],
        _ => "()",
    }
}

pub fn hover(parse: &Parse, names: &Names, text: &str, offset: TextSize, cx: FileCtx) -> Option<Hover> {
    let tok = name_token(parse, offset)?;
    let range = tok.text_range();
    let path = token_path(names, &tok);
    if let Some(path) = &path {
        let items = items_for(path, cx.game);
        if !items.is_empty() {
            return Some(Hover { range, markdown: render_items(&items) });
        }
    }
    // What another file or resource defines.
    if let Some((ws, file)) = cx.ws
        && let Some(target) = api_target(text, names, &tok)
    {
        let overlay = api::file_api_full(parse, text, names);
        if let Some(item) = api_lookup(ws, file, Some(&overlay), &target) {
            let prefix = match &target {
                ApiTarget::Export { resource, .. } => format!("exports['{resource}']:"),
                ApiTarget::Member { .. } => String::new(),
            };
            return Some(Hover { range, markdown: render_api(ws, &item, &prefix) });
        }
    }
    if let Some(path) = path {
        return (!path.contains('.')).then(|| Hover { range, markdown: format!("```lua\n(global) {path}\n```") });
    }
    let Symbol::Decl(di) = names.symbol_at(range.start())? else { return None };
    let d = &names.decls[di];
    let attrib = match d.attrib {
        Some(Attrib::Const) => " <const>",
        Some(Attrib::Close) => " <close>",
        None => "",
    };
    // As LuaLS writes them: `function f(a: any)` with its return lines,
    // `local x: integer = 1`, `(parameter) a: any`.
    let function = match (d.kind, d.function) {
        (DeclKind::LocalFunction, _) => Some(d.owner),
        // A `---@type` on the local wins over the function's own signature.
        (DeclKind::Local, Some(f)) if !leading_comments(text, d.owner).lines().any(|l| l.trim_start().starts_with("@type")) => Some(f),
        _ => None,
    };
    if let Some(body) = function.and_then(|r| function_body_in(parse, r)) {
        let (sig, rets, is_async) = crate::types::FileTypes::new(parse, names, text).signature(&body);
        let mut code = format!("{}function {}{sig}", if is_async { "(async) " } else { "" }, d.name);
        for line in rets {
            code.push('\n');
            code.push_str(&line);
        }
        let mut markdown = format!("```lua\n{code}\n```");
        let doc = leading_comments(text, d.owner);
        if !doc.is_empty() {
            markdown.push_str("\n\n");
            markdown.push_str(&render_doc(&doc));
        }
        return Some(Hover { range, markdown });
    }
    let code = match d.kind {
        DeclKind::Param => format!("(parameter) {}", d.name),
        DeclKind::SelfParam => "(self) self".to_string(),
        DeclKind::Label => format!("::{}::", d.name),
        _ => format!("local {}{attrib}", d.name),
    };
    let mut code = code;
    if matches!(d.kind, DeclKind::Local | DeclKind::LocalFunction | DeclKind::Param | DeclKind::ForVar) {
        let mut types = Vec::new();
        if let Some((ws, file)) = cx.ws
            && let Some(Symbol::Decl(i)) = names.symbol_at(range.start())
        {
            let overlay = api::file_api_full(parse, text, names);
            types = ws.type_views(file, Some(&overlay), &Ref::Local(i as u32));
        }
        // What the file alone tells: literals, operators, doc tags, narrowing.
        if types.is_empty()
            && let Some(t) = crate::types::type_at(parse, names, text, range.start())
        {
            types.push(t);
        }
        if types.is_empty() {
            types.push(if d.kind == DeclKind::Param { "any" } else { "unknown" }.to_string());
        }
        code.push_str(": ");
        code.push_str(&types.join("|"));
        if let Some(v) = literal_value(parse, text, d) {
            code.push_str(" = ");
            code.push_str(&v);
        }
        // The fields under it, as LuaLS lists them: a constructor's shape takes
        // the type's place (`local t: {`), a class's follow it (`local c: C {`).
        if let Some((fields, constructor)) = crate::types::FileTypes::new(parse, names, text).hover_fields(di) {
            if constructor {
                code = format!("local {}{attrib}: {{", d.name);
            } else {
                code.push_str(" {");
            }
            for f in fields {
                code.push_str("\n    ");
                code.push_str(&f);
                if f != "..." {
                    code.push(',');
                }
            }
            code.push_str("\n}");
        }
    }
    let mut markdown = format!("```lua\n{code}\n```");
    if matches!(d.kind, DeclKind::Local | DeclKind::LocalFunction) {
        let doc = leading_comments(text, d.owner);
        if !doc.is_empty() {
            markdown.push_str("\n\n");
            markdown.push_str(&render_doc(&doc));
        }
    }
    Some(Hover { range, markdown })
}

/// The expression that ends right before the `.` / `:` at `dot`.
fn expr_before(parse: &Parse, dot: usize) -> Option<SyntaxNode> {
    let mut end = TextSize::from(dot as u32);
    let mut tok = parse.syntax().token_at_offset(end).left_biased()?;
    // `t .`: spaces before the dot.
    if tok.kind() == WHITESPACE && tok.text_range().end() == end {
        tok = tok.prev_token()?;
        end = tok.text_range().end();
    }
    if tok.text_range().end() != end {
        return None;
    }
    let mut best = None;
    for a in tok.parent_ancestors() {
        if a.text_range().end() != end {
            break;
        }
        if matches!(a.kind(), NAME_EXPR | INDEX_EXPR | CALL_EXPR | METHOD_CALL_EXPR | PAREN_EXPR) {
            best = Some(a);
        }
    }
    best
}

/// Where a local can be seen.
pub(crate) fn scope(parse: &Parse, d: &crate::names::Decl) -> Option<TextRange> {
    match d.kind {
        DeclKind::ForVar | DeclKind::SelfParam => Some(d.owner),
        DeclKind::Param => {
            let tok = token_at(&parse.syntax(), d.range.start()).right_biased()?;
            tok.parent_ancestors().find(|n| n.kind() == FUNC_BODY).map(|b| b.text_range())
        }
        DeclKind::Local | DeclKind::LocalFunction => {
            let tok = token_at(&parse.syntax(), d.range.start()).right_biased()?;
            let stat = tok.parent_ancestors().find(|n| n.text_range() == d.owner)?;
            stat.parent().map(|b| b.text_range())
        }
        DeclKind::Label => None,
    }
}

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn side_fits(items: &[LibItem], side: FileSide, game: Game) -> bool {
    items.iter().any(|i| {
        (i.game == Game::Any || i.game == game)
            && !matches!(
                (i.origin, i.side, side),
                (Origin::Native, Side::Client, FileSide::Server) | (Origin::Native, Side::Server, FileSide::Client)
            )
    })
}

fn library_completion(label: &str, items: &[LibItem]) -> Completion {
    let lib = library();
    let first = &items[0];
    Completion {
        label: label.to_string(),
        kind: if first.function { CompletionKind::Function } else { CompletionKind::Global },
        detail: Some(lib.declaration(first)),
        deprecated: false,
        replace: None, insert: None, edits: Vec::new(),
    }
}

/// The resource of `exports.res` / `exports['res']` ending right before `end`.
fn exports_before(text: &str, end: usize) -> Option<String> {
    let s = text[..end].trim_end();
    if let Some(rest) = s.strip_suffix(']') {
        let rest = rest.trim_end();
        let q = rest.chars().last().filter(|c| matches!(c, '\'' | '"'))?;
        let body = &rest[..rest.len() - 1];
        let open = body.rfind(q)?;
        let name = &body[open + 1..];
        let head = body[..open].trim_end().strip_suffix('[')?.trim_end();
        return (head.ends_with("exports") && !name.is_empty()).then(|| name.to_string());
    }
    let name_start = s.bytes().rposition(|b| !is_ident(b)).map_or(0, |i| i + 1);
    let name = &s[name_start..];
    let head = s[..name_start].strip_suffix('.')?;
    let before = head.len().checked_sub("exports".len())?;
    let ok = head.ends_with("exports") && (before == 0 || !is_ident(head.as_bytes()[before - 1]));
    (ok && !name.is_empty()).then(|| name.to_string())
}

pub fn complete(parse: &Parse, names: &Names, text: &str, offset: TextSize, cx: FileCtx) -> Option<Completions> {
    complete_with(parse, names, text, offset, cx, CompleteOptions::default())
}

/// Completion with the client's trigger and the word setting: the items, then
/// the file's other words (LuaLS `checkCommon`) where `show_word` allows.
pub fn complete_with(parse: &Parse, names: &Names, text: &str, offset: TextSize, cx: FileCtx, opts: CompleteOptions) -> Option<Completions> {
    let got = complete_triggered(parse, names, text, offset, cx, opts.trigger);
    let end = usize::from(offset).min(text.len());
    let typed = text[..end].bytes().rev().take_while(|b| is_ident(*b)).count();
    let word = &text[end - typed..end];
    let tok = token_at(&parse.syntax(), offset).left_biased();
    let in_comment = tok.as_ref().is_some_and(|t| matches!(t.kind(), COMMENT | BLOCK_COMMENT) && t.text_range().start() < offset);
    let in_string = tok.as_ref().is_some_and(|t| matches!(t.kind(), STRING | LONG_STRING) && t.text_range().start() < offset && t.text_range().end() > offset);
    let has_space = opts.trigger.is_some() && word.is_empty() && text[..end].ends_with([' ', '\t']);
    let empty = got.as_ref().is_none_or(|c| c.items.is_empty());
    let words = match opts.show_word {
        ShowWord::Disable => false,
        ShowWord::Fallback => empty,
        ShowWord::Enable => empty || (!word.is_empty() && !in_string),
    };
    // After `(`, `,`, `{`, `#` LuaLS looks for no words.
    if !words || in_comment || has_space || matches!(opts.trigger, Some('(' | ',' | '{' | '#')) {
        return got;
    }
    let had = got.is_some();
    let mut out = got.unwrap_or(Completions { items: Vec::new(), incomplete: false });
    let mut used: HashSet<String> = out.items.iter().map(|i| i.label.split('(').next().unwrap_or("").to_string()).collect();
    used.extend(KEYWORDS.iter().map(|k| k.to_string()));
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !(bytes[i].is_ascii_alphabetic() || bytes[i] == b'_') || (i > 0 && is_ident(bytes[i - 1])) {
            i += 1;
            continue;
        }
        let s = i;
        while i < bytes.len() && is_ident(bytes[i]) {
            i += 1;
        }
        if out.items.len() >= 100 {
            out.incomplete = true;
            break;
        }
        let w = &text[s..i];
        // Not the word being typed.
        if w.len() >= 3 && i != end && !used.contains(w) {
            used.insert(w.to_string());
            if crate::doc_complete::matchkey(word, w) {
                out.items.push(Completion { label: w.to_string(), kind: CompletionKind::Text, detail: None, deprecated: false, replace: None, insert: None, edits: Vec::new() });
            }
        }
    }
    (had || !out.items.is_empty()).then_some(out)
}

/// Completion the client asked for after typing `trigger` (LuaLS
/// `tryWord`): after `(`, `,`, `{`, `#` or a space only the literal values and
/// table keys the position takes, and fields after `t.`, no names.
fn complete_triggered(parse: &Parse, names: &Names, text: &str, offset: TextSize, cx: FileCtx, trigger: Option<char>) -> Option<Completions> {
    let (game, side) = (cx.game, cx.side);
    let end = usize::from(offset).min(text.len());
    let bytes = text.as_bytes();
    if let Some(items) = crate::doc_complete::complete(parse, names, text, offset, cx) {
        return Some(Completions { items, incomplete: false });
    }
    // The literal values the position takes: alone in a string or before
    // anything is typed, first otherwise.
    let values = crate::doc_complete::values(parse, text, offset);
    if let Some((items, true)) = values {
        return Some(Completions { items, incomplete: false });
    }
    // No completion inside comments and strings.
    if let Some(t) = token_at(&parse.syntax(), offset).left_biased()
        && matches!(t.kind(), COMMENT | BLOCK_COMMENT | STRING | LONG_STRING)
        && t.text_range().start() < offset
        && (t.text_range().end() > offset || t.kind() == COMMENT)
    {
        return None;
    }
    // While naming a parameter (`function f(a, |)`): names, the function's
    // `---@param` names, no keywords.
    let param_list = token_at(&parse.syntax(), offset)
        .left_biased()
        .filter(|t| t.kind() != R_PAREN)
        .and_then(|t| t.parent_ancestors().find(|a| a.kind() == PARAM_LIST));
    let mut start = end;
    while start > 0 && is_ident(bytes[start - 1]) {
        start -= 1;
    }
    let prefix = &text[start..end];
    if prefix.as_bytes().first().is_some_and(u8::is_ascii_digit) {
        return None;
    }
    let member_ahead = text[..start].trim_end_matches([' ', '\t']).ends_with(['.', ':']);
    if matches!(trigger, Some('(' | ',' | '{' | '#' | ' ' | '\t')) && prefix.is_empty() && !member_ahead && param_list.is_none() {
        return None;
    }
    // LuaLS's match: a prefix, or the typed letters in order of words (`zac` for `zabc`).
    let matches = |label: &str| crate::doc_complete::matchkey(prefix, label);
    // The library holds thousands of natives: a plain prefix there.
    let lower = prefix.to_ascii_lowercase();
    let lib_matches = |label: &str| label.to_ascii_lowercase().starts_with(&lower);
    let mut items: Vec<Completion> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut push = |items: &mut Vec<Completion>, c: Completion| {
        if seen.insert(c.label.clone()) {
            items.push(c);
        }
    };
    for c in values.map(|(v, _)| v).unwrap_or_default() {
        push(&mut items, c);
    }

    // `obj.` / `obj:`: the library's fields of `obj`, and the fields the file uses on it.
    // Spaces may stand between the dot and the cursor (`t.   |`).
    let sep = if prefix.is_empty() { text[..start].trim_end_matches([' ', '\t']).len() } else { start };
    let member = sep > 0 && matches!(bytes[sep - 1], b'.' | b':') && !(sep > 1 && bytes[sep - 2] == b'.');
    // `exports.res:` / `exports['res']:`: the resource's exports on this side.
    if member
        && bytes[sep - 1] == b':'
        && let Some((ws, file)) = cx.ws
        && let Some(resource) = exports_before(text, sep - 1)
    {
        let e = ws.exports_of(file, &resource)?;
        for i in e.items.iter().filter(|i| matches(&i.name)) {
            push(&mut items, api_completion(i));
        }
        let incomplete = items.len() > MAX_ITEMS;
        items.truncate(MAX_ITEMS);
        return Some(Completions { items, incomplete });
    }
    if member {
        let mut o = text[..sep - 1].trim_end_matches([' ', '\t']).len();
        let object_end = o;
        while o > 0 && (is_ident(bytes[o - 1]) || bytes[o - 1] == b'.') {
            o -= 1;
        }
        let object = &text[o..object_end];
        let plain = !(object.is_empty() || object.split('.').any(|s| s.is_empty() || s.as_bytes()[0].is_ascii_digit()));
        // `s:` / `s.` on a string (`('%s'):format`, `local s = ''`): the string library.
        if let Some(e) = expr_before(parse, sep - 1)
            && crate::types::FileTypes::new(parse, names, text).is_string(&e)
        {
            let mut lib_items: Vec<(&str, &[LibItem])> =
                library().names().filter(|(n, _)| n.strip_prefix("string.").is_some_and(|rest| !rest.contains('.') && matches(rest))).collect();
            lib_items.sort_by_key(|(n, _)| *n);
            for (n, list) in lib_items {
                push(&mut items, library_completion(&n["string.".len()..], list));
            }
            return Some(Completions { items, incomplete: false });
        }
        // The fields its type has (LuaLS kinds: a literal is Enum), without the
        // private and protected ones it may not see.
        let colon = bytes[sep - 1] == b':';
        let mut hidden: Vec<String> = Vec::new();
        if let Some(e) = expr_before(parse, sep - 1) {
            let (members, h) = crate::types::FileTypes::new(parse, names, text).member_items(&e);
            hidden = h;
            for (name, kind) in members {
                if !matches(&name) || (colon && kind != crate::types::MemberKind::Function && kind != crate::types::MemberKind::Field) {
                    continue;
                }
                let kind = match kind {
                    crate::types::MemberKind::Field => CompletionKind::Field,
                    crate::types::MemberKind::Enum => CompletionKind::Enum,
                    crate::types::MemberKind::Function => CompletionKind::Function,
                };
                // A name that is not an identifier: `'a.b.c'`.
                let label = if crate::names::is_name(&name) || name.parse::<i64>().is_ok() { name } else { format!("'{name}'") };
                push(&mut items, Completion { label, kind, detail: None, deprecated: false, replace: None, insert: None, edits: Vec::new() });
            }
        }
        // Any expression: `GetEntityCoords(ped).`, `ESX.GetPlayerFromId(src).job.`.
        let expr = cx.ws.and_then(|_| expr_before(parse, sep - 1));
        if !plain && expr.is_none() {
            return None;
        }
        let dotted = format!("{object}.");
        let mut lib_items: Vec<(&str, &[LibItem])> = library()
            .names()
            .filter(|(n, _)| plain && n.strip_prefix(&dotted).is_some_and(|rest| !rest.contains('.') && matches(rest)))
            .collect();
        lib_items.sort_by_key(|(n, _)| *n);
        for (n, list) in lib_items {
            push(&mut items, library_completion(&n[dotted.len()..], list));
        }
        if let (Some((ws, file)), Some(e)) = (cx.ws, &expr) {
            let (overlay, r) = api::expr_ref(parse, text, names, e);
            for i in ws.api_members(file, Some(&overlay), &r, &[]) {
                if matches(&i.name) && (!colon || i.params.is_some()) {
                    push(&mut items, api_completion(&i));
                }
            }
        }
        if !plain {
            let incomplete = items.len() > MAX_ITEMS;
            items.truncate(MAX_ITEMS);
            return Some(Completions { items, incomplete });
        }
        if let Some((ws, file)) = cx.ws {
            if object == "exports" && bytes[sep - 1] == b'.' {
                // Resources with exports (names that are Lua identifiers).
                for r in ws.export_resources().into_iter().filter(|r| crate::names::is_name(r) && matches(r)) {
                    push(&mut items, Completion { label: r, kind: CompletionKind::Field, detail: Some("resource".into()), deprecated: false, replace: None, insert: None, edits: Vec::new() });
                }
            } else if expr.is_none() {
                // Fields another file or resource defines (`QBCore.Functions.`, `lib.`)
                // when the syntax tree has no expression before the dot.
                let mut parts = object.split('.');
                let first = parts.next().unwrap_or("");
                let root = match names.symbol_at(TextSize::from(o as u32)) {
                    Some(Symbol::Decl(d)) => Some(Ref::Local(d as u32)),
                    Some(Symbol::Global(_)) => Some(Ref::Global(first.to_string())),
                    None => None,
                };
                if let Some(root) = root {
                    let fields: Vec<String> = parts.map(str::to_string).collect();
                    let overlay = api::file_api_full(parse, text, names);
                    for i in ws.api_members(file, Some(&overlay), &root, &fields) {
                        // After `:` only functions make sense.
                        if matches(&i.name) && (!colon || i.params.is_some()) {
                            push(&mut items, api_completion(&i));
                        }
                    }
                }
            }
        }
        // Fields written or read on the same object elsewhere in the file (`Config.x`).
        let mut at = 0;
        while let Some(i) = text[at..].find(dotted.as_str()) {
            let s = at + i;
            at = s + dotted.len();
            let before_ok = s == 0 || !(is_ident(bytes[s - 1]) || bytes[s - 1] == b'.');
            let field: String = text[at..].bytes().take_while(|&b| is_ident(b)).map(char::from).collect();
            if before_ok && !field.is_empty() && at != start && matches(&field) && !field.as_bytes()[0].is_ascii_digit() {
                // Written here (`x.f = v`): a field, its value's kind; only read: a word.
                let rest = text[at + field.len()..].trim_start_matches([' ', '\t']);
                let value = rest.strip_prefix('=').filter(|r| !r.starts_with('=')).map(str::trim_start);
                let kind = match value {
                    Some(v) if v.starts_with("function") => CompletionKind::Function,
                    Some(v) if v.starts_with(|c: char| c.is_ascii_digit() || c == '"' || c == '\'') || v.starts_with("true") || v.starts_with("false") => {
                        CompletionKind::Enum
                    }
                    Some(_) => CompletionKind::Field,
                    None => CompletionKind::Text,
                };
                // After `:` only functions.
                if colon && kind != CompletionKind::Function {
                    continue;
                }
                push(&mut items, Completion { label: field, kind, detail: None, deprecated: false, replace: None, insert: None, edits: Vec::new() });
            }
        }
        items.retain(|i| !hidden.contains(&i.label));
        // A field named `'a.b'` is not also offered bare.
        let quoted: Vec<String> = items.iter().filter_map(|i| i.label.strip_prefix('\'').and_then(|l| l.strip_suffix('\'')).map(str::to_string)).collect();
        items.retain(|i| !quoted.contains(&i.label));
        let incomplete = items.len() > MAX_ITEMS;
        items.truncate(MAX_ITEMS);
        return Some(Completions { items, incomplete });
    }

    // Naming a parameter or a new local (LuaLS `checkProvideLocal`): the names
    // the function reads further on, and the documented parameter names.
    let after_local = text[..start].trim_end().strip_suffix("local").is_some_and(|b| b.is_empty() || !is_ident(*b.as_bytes().last().unwrap_or(&b' ')));
    if param_list.is_some() || after_local {
        let block = token_at(&parse.syntax(), offset)
            .left_biased()
            .and_then(|t| t.parent_ancestors().find(|a| matches!(a.kind(), FUNC_BODY | SOURCE_FILE)))
            .unwrap_or_else(|| parse.syntax());
        let reach = block.text_range();
        for r in names.refs.iter().filter(|r| r.range.start() > offset && reach.contains_range(r.range) && r.access != crate::names::Access::Write) {
            let n = &text[r.range];
            if matches(n) && n != prefix {
                push(&mut items, Completion { label: n.to_string(), kind: CompletionKind::Local, detail: None, deprecated: false, replace: None, insert: None, edits: Vec::new() });
            }
        }
        if let Some(list) = &param_list {
            for c in crate::doc_complete::documented_params(text, list, prefix) {
                push(&mut items, c);
            }
        }
        if after_local && matches("function") {
            push(&mut items, Completion { label: "function".into(), kind: CompletionKind::Keyword, detail: None, deprecated: false, replace: None, insert: None, edits: Vec::new() });
        }
        return (!items.is_empty()).then_some(Completions { items, incomplete: false });
    }
    // Locals in scope, innermost first.
    let mut locals: Vec<&crate::names::Decl> = names
        .decls
        .iter()
        .filter(|d| d.kind != DeclKind::Label && d.range.end() <= offset && matches(&d.name))
        .filter(|d| scope(parse, d).is_some_and(|s| s.contains_inclusive(offset)))
        .collect();
    locals.sort_by_key(|d| std::cmp::Reverse(d.range.start()));
    for d in locals {
        let kind = if d.kind == DeclKind::LocalFunction || d.function.is_some() {
            CompletionKind::Function
        } else {
            CompletionKind::Local
        };
        push(&mut items, Completion { label: d.name.clone(), kind, detail: None, deprecated: false, replace: None, insert: None, edits: Vec::new() });
    }
    // Globals the file uses (its own and the resource's: `Config`, `QBCore`).
    let mut globals: Vec<&str> = names
        .refs
        .iter()
        .filter(|r| r.target == Target::Global && !r.range.contains_inclusive(offset))
        .map(|r| &text[r.range])
        .filter(|n| matches(n) && library().get(n).is_empty())
        .collect();
    globals.sort_unstable();
    globals.dedup();
    let root = parse.syntax();
    for g in globals {
        push(&mut items, Completion { label: g.to_string(), kind: global_kind(&root, text, g), detail: None, deprecated: false, replace: None, insert: None, edits: Vec::new() });
    }
    // `_G['a.b'] = x`: a global no name can reach, written quoted.
    if text.contains("_G[") {
        for e in root.descendants().filter(|e| e.kind() == INDEX_EXPR) {
            if e.first_child().is_some_and(|o| o.kind() == NAME_EXPR && o.text() == "_G")
                && let Some(t) = e.children().nth(1).filter(|k| k.kind() == LITERAL_EXPR).and_then(|k| k.first_token()).filter(|t| t.kind() == STRING)
                && !t.text_range().contains_inclusive(offset)
            {
                let name = crate::util::lua_string(t.text());
                let label = if crate::names::is_name(&name) { name.clone() } else { format!("'{name}'") };
                if matches(&name) {
                    push(&mut items, Completion { label, kind: CompletionKind::Global, detail: None, deprecated: false, replace: None, insert: None, edits: Vec::new() });
                }
            }
        }
    }
    for k in KEYWORDS.iter().filter(|k| matches(k)) {
        push(&mut items, Completion { label: k.to_string(), kind: CompletionKind::Keyword, detail: None, deprecated: false, replace: None, insert: None, edits: Vec::new() });
    }
    // `continue` in a loop: `goto continue` and a `::continue::` label before
    // its `end` (LuaLS).
    if !prefix.is_empty() && matches("continue") {
        for c in continue_items(parse, text, offset, start) {
            push(&mut items, c);
        }
    }
    // The library: thousands of natives, so only once a letter is typed.
    let mut incomplete = prefix.is_empty();
    if !prefix.is_empty() {
        let mut lib_items: Vec<(&str, &[LibItem])> = library()
            .names()
            .filter(|(n, list)| !n.contains('.') && lib_matches(n) && side_fits(list, side, game))
            .collect();
        lib_items.sort_by(|a, b| (a.0.len(), a.0).cmp(&(b.0.len(), b.0)));
        for (n, list) in lib_items {
            push(&mut items, library_completion(n, list));
        }
    }
    if items.len() > MAX_ITEMS {
        items.truncate(MAX_ITEMS);
        incomplete = true;
    }
    Some(Completions { items, incomplete })
}

pub fn signature_help(parse: &Parse, names: &Names, text: &str, offset: TextSize, cx: FileCtx) -> Option<SignatureHelp> {
    // From the last real token: in an unclosed `f(a, ` the whitespace after the
    // comma may belong to the next statement.
    // (`token_at`, not rowan's `token_at_offset` / `prev_token`: they recurse per
    // tree level.)
    let root = parse.syntax();
    let mut tok = token_at(&root, offset).left_biased()?;
    while tok.kind().is_trivia() {
        let start = tok.text_range().start();
        if start == TextSize::from(0) {
            return None;
        }
        tok = token_at(&root, start).left_biased()?;
    }
    let args = tok.parent_ancestors().find(|n| {
        let token = |e: Option<cfx_lua_syntax::SyntaxElement>| e.and_then(|e| e.into_token());
        n.kind() == ARG_LIST
            && token(n.first_child_or_token()).is_some_and(|t| t.kind() == L_PAREN && t.text_range().end() <= offset)
            && token(n.last_child_or_token()).is_none_or(|t| t.kind() != R_PAREN || t.text_range().start() >= offset)
    })?;
    let call = args.parent()?;
    let lib = library();
    let mut signatures: Vec<Signature> = match call.kind() {
        CALL_EXPR => expr_path(names, &call.first_child()?)
            .map(|path| {
                items_for(&path, cx.game)
                    .into_iter()
                    .filter(|i| i.function)
                    .map(|i| Signature {
                        label: lib.declaration(i).trim_start_matches("function ").to_string(),
                        params: lib.params(i),
                        doc: render_doc(&lib.doc(i)),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    if signatures.is_empty()
        && let Some((ws, file)) = cx.ws
    {
        let target = match call.kind() {
            CALL_EXPR => chain(text, names, &call.first_child()?).map(|(root, fields)| ApiTarget::Member { root, fields }),
            METHOD_CALL_EXPR => api::method_name(&call).and_then(|m| api_target(text, names, &m)),
            _ => None,
        }?;
        let overlay = api::file_api_full(parse, text, names);
        let item = api_lookup(ws, file, Some(&overlay), &target)?;
        let params = item.params.clone()?;
        signatures.push(Signature {
            label: item.signature(),
            params: params.into_iter().map(|p| (p, None)).collect(),
            doc: render_doc(&item.doc),
        });
    }
    if signatures.is_empty() {
        return None;
    }
    let active_param = args
        .children_with_tokens()
        .filter(|e| e.kind() == COMMA && e.text_range().end() <= offset)
        .count();
    Some(SignatureHelp { signatures, active_param })
}

/// Where another file or resource defines the field, global or export at the
/// offset: the file and the name's byte offset in it.
pub fn api_definition(parse: &Parse, names: &Names, text: &str, offset: TextSize, ws: &Workspace, file: &Path) -> Option<(std::path::PathBuf, u32)> {
    let tok = name_token(parse, offset)?;
    let target = api_target(text, names, &tok)?;
    let overlay = api::file_api_full(parse, text, names);
    api_lookup(ws, file, Some(&overlay), &target)?.location
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{names, parse};

    fn at(text: &str) -> (String, TextSize) {
        let i = text.find('$').unwrap();
        (text.replacen('$', "", 1), TextSize::from(i as u32))
    }

    fn labels(text: &str, side: FileSide) -> (Vec<String>, bool) {
        let (t, o) = at(text);
        let p = parse(&t);
        let n = names(&p, &t);
        let c = complete(&p, &n, &t, o, FileCtx::plain(Game::Gta5, side)).unwrap();
        (c.items.into_iter().map(|i| i.label).collect(), c.incomplete)
    }

    #[test]
    fn hovers() {
        let (t, o) = at("local p = GetEntity$Coords(PlayerPedId())\n");
        let p = parse(&t);
        let n = names(&p, &t);
        let h = hover(&p, &n, &t, o, FileCtx::plain(Game::Gta5, FileSide::Unknown)).unwrap();
        assert!(h.markdown.contains("function GetEntityCoords("), "{}", h.markdown);
        assert_eq!(&t[h.range], "GetEntityCoords");
        let (t, o) = at("local s = string.for$mat('%d', 1)\n");
        let p = parse(&t);
        let n = names(&p, &t);
        assert!(hover(&p, &n, &t, o, FileCtx::plain(Game::Gta5, FileSide::Unknown)).unwrap().markdown.contains("function string.format("));
        let (t, o) = at("-- The speed limit.\nlocal LIMIT <const> = 5\nprint(LIM$IT)\nlocal function add(a, b) return a + b end\nprint(add)\n");
        let p = parse(&t);
        let n = names(&p, &t);
        let h = hover(&p, &n, &t, o, FileCtx::plain(Game::Gta5, FileSide::Unknown)).unwrap();
        assert!(h.markdown.contains("local LIMIT <const>") && h.markdown.contains("The speed limit."), "{}", h.markdown);
        let o = TextSize::from(t.rfind("add").unwrap() as u32);
        let h = hover(&p, &n, &t, o, FileCtx::plain(Game::Gta5, FileSide::Unknown)).unwrap().markdown;
        assert!(h.contains("function add(a: any, b: any)"), "{h}");
    }

    #[test]
    fn completions() {
        let (l, incomplete) = labels("local playerName = 1\nlocal function f(param)\n  print(pa$)\nend\n", FileSide::Unknown);
        assert!(l.contains(&"param".to_string()) && l.contains(&"pairs".to_string()), "{l:?}");
        assert!(!l.contains(&"playerName".to_string()) || l.iter().position(|x| x == "param") < l.iter().position(|x| x == "pairs"));
        assert!(!incomplete);
        // Out of scope: a local of another function.
        // (LuaLS still offers it as a word of the file.)
        let (t, o) = at("local function a() local inner = 1 end\nlocal function b() print(inn$) end\n");
        let p = parse(&t);
        let n = names(&p, &t);
        let c = complete(&p, &n, &t, o, FileCtx::plain(Game::Gta5, FileSide::Unknown)).unwrap();
        assert!(c.items.iter().all(|i| i.label != "inner" || i.kind == CompletionKind::Text), "{:?}", c.items);
        // A string's methods; nothing while naming a parameter.
        let (l, _) = labels("local s = ('%d'):for$\n", FileSide::Unknown);
        assert_eq!(l, ["format"]);
        let (l, _) = labels("local name = 'x'\nprint(name:up$)\n", FileSide::Unknown);
        assert_eq!(l, ["unpack", "upper"]);
        let (t, o) = at("local function f(a, $)\nend\n");
        let p = parse(&t);
        let n = names(&p, &t);
        assert!(complete(&p, &n, &t, o, FileCtx::plain(Game::Gta5, FileSide::Unknown)).is_none());
        // Members: the library's and the fields the file uses.
        let (l, _) = labels("Config = {}\nConfig.maxPlayers = 4\nprint(string.fo$)\n", FileSide::Unknown);
        assert_eq!(l, ["format"]);
        let (l, _) = labels("Config = {}\nConfig.maxPlayers = 4\nprint(Config.$)\n", FileSide::Unknown);
        assert_eq!(l, ["maxPlayers"]);
        // Server files get no client-only natives.
        let (l, _) = labels("PlayerPed$\n", FileSide::Server);
        assert!(!l.contains(&"PlayerPedId".to_string()), "{l:?}");
        let (l, _) = labels("PlayerPed$\n", FileSide::Client);
        assert!(l.contains(&"PlayerPedId".to_string()));
        // Nothing in comments and strings.
        let (t, o) = at("-- Play$\nlocal s = 'Play$'\n");
        let p = parse(&t);
        let n = names(&p, &t);
        assert!(complete(&p, &n, &t, o, FileCtx::plain(Game::Gta5, FileSide::Client)).is_none());
    }

    /// A 20,000-level chain (valid Lua) must not overflow the stack.
    #[test]
    fn deep_chains() {
        let mut t = String::from("x = a");
        t.push_str(&".b".repeat(20_000));
        t.push_str("\nf(a");
        t.push_str(&"(1)".repeat(20_000));
        t.push_str(", ");
        let p = parse(&t);
        let n = names(&p, &t);
        let field = TextSize::from(t.find("\nf").unwrap() as u32 - 1);
        assert!(hover(&p, &n, &t, field, FileCtx::plain(Game::Gta5, FileSide::Unknown)).is_none());
        let end = TextSize::from(t.len() as u32);
        assert!(signature_help(&p, &n, &t, end, FileCtx::plain(Game::Gta5, FileSide::Unknown)).is_none());
        let c = complete(&p, &n, &t, field, FileCtx::plain(Game::Gta5, FileSide::Client));
        assert!(c.is_some());
    }

    #[test]
    fn signatures() {
        let (t, o) = at("SetEntityCoords(ped, $\nlocal x = 1\n");
        let p = parse(&t);
        let n = names(&p, &t);
        assert_eq!(signature_help(&p, &n, &t, o, FileCtx::plain(Game::Gta5, FileSide::Unknown)).unwrap().active_param, 1);
        let (t, o) = at("SetEntityCoords(ped, 1.0, $)\n");
        let p = parse(&t);
        let n = names(&p, &t);
        let s = signature_help(&p, &n, &t, o, FileCtx::plain(Game::Gta5, FileSide::Unknown)).unwrap();
        assert_eq!(s.active_param, 2);
        let sig = &s.signatures[0];
        assert!(sig.label.starts_with("SetEntityCoords(entity"), "{}", sig.label);
        assert_eq!(sig.params[0], ("entity".to_string(), Some("integer".to_string())));
    }
}
