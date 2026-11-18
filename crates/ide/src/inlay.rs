//! Inlay hints: the parameter name before each argument of a call whose
//! parameters are known (natives, the Lua and CfxLua library, functions of the
//! file and of the workspace), and the type after a local whose value does not
//! show it (`local xPlayer = ESX.GetPlayerFromId(src)` gets `: xPlayer`).
//!
//! Questions to the workspace stop after [`WORKSPACE_BUDGET`] per request; what
//! is left gets what the file alone tells. One callee or local is asked once.

use crate::api;
use crate::assist::{ApiTarget, FileCtx, api_lookup, chain, expr_path, items_for, param_list};
use crate::library::library;
use crate::names::{DeclKind, Names, Target};
use crate::types::FileTypes;
use crate::{Parse, TextRange, TextSize};
use cfx_lua_syntax::SyntaxKind::*;
use cfx_lua_syntax::SyntaxNode;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Hints given for one range at most.
pub const MAX_INLAY_HINTS: usize = 500;

/// Time one request may spend asking the workspace (debug builds are tens of
/// times slower).
const WORKSPACE_BUDGET: Duration = Duration::from_millis(if cfg!(debug_assertions) { 2000 } else { 40 });

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlayKind {
    /// `name:` before an argument.
    Parameter,
    /// `: type` after a local's name.
    Type,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlayHint {
    pub offset: TextSize,
    pub label: String,
    pub kind: InlayKind,
}

struct Hints<'a> {
    parse: &'a Parse,
    names: &'a Names,
    text: &'a str,
    cx: FileCtx<'a>,
    types: FileTypes<'a>,
    overlay: Option<api::FileApi>,
    /// Parameters by callee text (`numStr:sub`, `ESX.GetPlayerFromId`).
    params: HashMap<String, Option<Vec<String>>>,
    start: Instant,
    out: Vec<InlayHint>,
}

pub fn inlay_hints(parse: &Parse, names: &Names, text: &str, range: TextRange, cx: FileCtx) -> Vec<InlayHint> {
    let mut h = Hints {
        parse,
        names,
        text,
        cx,
        types: FileTypes::new(parse, names, text),
        overlay: None,
        params: HashMap::new(),
        start: Instant::now(),
        out: Vec::new(),
    };
    let mut todo = vec![parse.syntax()];
    while let Some(n) = todo.pop() {
        if h.out.len() >= MAX_INLAY_HINTS {
            break;
        }
        let r = n.text_range();
        if r.end() < range.start() || r.start() > range.end() {
            continue;
        }
        match n.kind() {
            ARG_LIST => h.param_hints(&n),
            LOCAL_STAT => h.type_hints(&n),
            _ => {}
        }
        // Children in order: pushed reversed.
        let kids: Vec<SyntaxNode> = n.children().collect();
        todo.extend(kids.into_iter().rev());
    }
    let mut out = h.out;
    out.sort_by_key(|h| h.offset);
    out.truncate(MAX_INLAY_HINTS);
    out
}

/// A type label worth showing: `unknown` and `any` members dropped, nothing
/// when only `nil` is left or a generic is unbound (`<T>`), a long table shape
/// shown as `table`, anything else cut at 60 characters.
fn tidy(label: String) -> Option<String> {
    // Top-level `|` members.
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0i32, 0usize);
    for (i, c) in label.char_indices() {
        match c {
            '(' | '{' | '<' | '[' => depth += 1,
            ')' | '}' | '>' | ']' => depth -= 1,
            '|' if depth == 0 => {
                parts.push(label[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(label[start..].trim());
    let kept: Vec<&str> = parts.into_iter().filter(|p| !matches!(*p, "unknown" | "any" | "")).collect();
    if kept.iter().all(|p| *p == "nil") || kept.iter().any(|p| p.starts_with('<')) {
        return None;
    }
    let label = kept.join("|");
    if label.starts_with('{') && label.chars().count() > 40 {
        return Some("table".to_string());
    }
    if label.chars().count() > 60 {
        return Some(label.chars().take(59).collect::<String>() + "…");
    }
    Some(label)
}

fn split_params(list: &str) -> Vec<String> {
    list.trim_start_matches('(').trim_end_matches(')').split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect()
}

impl<'a> Hints<'a> {
    /// The workspace and file, while the budget lasts.
    fn ws(&self) -> Option<(&'a crate::workspace::Workspace, &'a std::path::Path)> {
        self.cx.ws.filter(|_| self.start.elapsed() < WORKSPACE_BUDGET)
    }

    fn overlay(&mut self) -> &api::FileApi {
        let (parse, text, names) = (self.parse, self.text, self.names);
        self.overlay.get_or_insert_with(|| api::file_api_full(parse, text, names))
    }

    /// The parameter names of the function a call calls.
    fn call_params(&mut self, call: &SyntaxNode) -> Option<Vec<String>> {
        let callee = call.first_child()?;
        let key = match call.kind() {
            METHOD_CALL_EXPR => format!("{}:{}", &self.text[callee.text_range()], api::method_name(call)?.text()),
            _ => self.text[callee.text_range()].to_string(),
        };
        if let Some(known) = self.params.get(&key) {
            return known.clone();
        }
        let found = self.find_params(call, &callee);
        // A miss for want of time is not kept: a later request may have it.
        if found.is_some() || self.ws().is_some() {
            self.params.insert(key, found.clone());
        }
        found
    }

    fn find_params(&mut self, call: &SyntaxNode, callee: &SyntaxNode) -> Option<Vec<String>> {
        let (names, text) = (self.names, self.text);
        if call.kind() == CALL_EXPR {
            // A local function of the file.
            if callee.kind() == NAME_EXPR
                && let Some(tok) = callee.first_token()
                && let Ok(i) = names.refs.binary_search_by_key(&tok.text_range().start(), |r| r.range.start())
                && let Target::Decl(d) = names.refs[i].target
            {
                let d = &names.decls[d];
                let list = match (d.kind, d.function) {
                    (DeclKind::LocalFunction, _) => Some(param_list(text, d.owner)),
                    (DeclKind::Local, Some(f)) => Some(param_list(text, f)),
                    _ => None,
                };
                if let Some(list) = list {
                    return Some(split_params(list));
                }
            }
            if let Some(path) = expr_path(names, callee) {
                let lib = library();
                if let Some(item) = items_for(&path, self.cx.game).into_iter().find(|i| i.function) {
                    return Some(lib.params(item).into_iter().map(|(p, _)| p).collect());
                }
            }
        }
        let (ws, file) = self.ws()?;
        let target = match call.kind() {
            CALL_EXPR => chain(text, names, callee).map(|(root, fields)| ApiTarget::Member { root, fields }),
            METHOD_CALL_EXPR => api::method_name(call).and_then(|m| crate::assist::api_target(text, names, &m)),
            _ => None,
        }?;
        let overlay = self.overlay.get_or_insert_with(|| api::file_api_full(self.parse, text, names));
        let item = api_lookup(ws, file, Some(overlay), &target)?;
        let mut params = item.params?;
        // `obj:f(a)` for `function obj.f(self, a)`, `obj.f(o, a)` for `function obj:f(a)`.
        match (call.kind(), item.method) {
            (METHOD_CALL_EXPR, false) if params.first().is_some_and(|p| p == "self") => {
                params.remove(0);
            }
            (CALL_EXPR, true) => params.insert(0, "self".to_string()),
            _ => {}
        }
        Some(params)
    }

    fn param_hints(&mut self, args: &SyntaxNode) {
        let Some(call) = args.parent() else { return };
        let values: Vec<SyntaxNode> = args.children().collect();
        // `f{...}` and `f'...'` have no parentheses to put names in.
        if values.is_empty() || args.first_token().is_none_or(|t| t.kind() != L_PAREN) {
            return;
        }
        let Some(params) = self.call_params(&call) else { return };
        for (v, p) in values.iter().zip(&params) {
            let p = p.trim_end_matches('?');
            if p == "..." || p.is_empty() || p == "_" {
                break;
            }
            // `f(name)` for parameter `name` says it already.
            let arg = self.text[v.text_range()].trim();
            let last = arg.rsplit(['.', ':']).next().unwrap_or(arg);
            if last.eq_ignore_ascii_case(p) {
                continue;
            }
            self.out.push(InlayHint { offset: v.text_range().start(), label: format!("{p}:"), kind: InlayKind::Parameter });
        }
    }

    fn type_hints(&mut self, stat: &SyntaxNode) {
        let locals: Vec<SyntaxNode> = stat.children().filter(|c| c.kind() == LOCAL_NAME).collect();
        let values: Vec<SyntaxNode> = stat.children().find(|c| c.kind() == EXPR_LIST).map(|l| l.children().collect()).unwrap_or_default();
        // `---@type` / `---@class` above say the type already.
        let tagged = stat
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .any(|t| t.kind() == COMMENT && (t.text().contains("@type") || t.text().contains("@class")));
        if tagged {
            return;
        }
        for (i, l) in locals.iter().enumerate() {
            let Some(tok) = l.first_token() else { continue };
            if tok.text() == "_" {
                continue;
            }
            // Literals, tables and functions show their type.
            let value = values.get(i).or(values.last().filter(|v| matches!(v.kind(), CALL_EXPR | METHOD_CALL_EXPR)));
            let Some(value) = value else { continue };
            if matches!(value.kind(), LITERAL_EXPR | TABLE_EXPR | FUNCTION_EXPR) {
                continue;
            }
            let Some(d) = self.names.decls.iter().position(|d| d.range == tok.text_range()) else { continue };
            let mut views = Vec::new();
            if let Some((ws, file)) = self.ws() {
                self.overlay();
                views = ws.type_views(file, self.overlay.as_ref(), &api::Ref::Local(d as u32));
            }
            let label = if views.is_empty() { self.types.decl(d) } else { Some(views.join("|")) };
            let Some(label) = label.and_then(tidy) else { continue };
            self.out.push(InlayHint { offset: tok.text_range().end(), label: format!(": {label}"), kind: InlayKind::Type });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::Game;
    use crate::manifest::FileSide;

    fn hints(text: &str) -> Vec<String> {
        let p = crate::parse(text);
        let n = crate::names(&p, text);
        let all = TextRange::new(0.into(), TextSize::from(text.len() as u32));
        inlay_hints(&p, &n, text, all, FileCtx::plain(Game::Gta5, FileSide::Client))
            .into_iter()
            .map(|h| format!("{}@{}", h.label, text[usize::from(h.offset)..].chars().take(3).collect::<String>()))
            .collect()
    }

    #[test]
    fn names_and_types() {
        let h = hints("local ped = PlayerPedId()\nSetEntityCoords(ped, 1.0, 2.0, 3.0, false, false, false, true)\n");
        assert!(h.iter().any(|h| h.starts_with("xPos:@1.0")), "{h:?}");
        assert!(h.iter().any(|h| h.starts_with("entity:@ped")), "{h:?}");
        let h = hints("local function add(a, b) return 1 end\nlocal s = add(1, 2)\nlocal t = s * 2.5\nadd(a, 3)\n");
        assert_eq!(h, [": integer@ = ", "a:@1, ", "b:@2)\n", ": number@ = ", "b:@3)\n"], "{h:?}");
        let h = hints("local c = GetEntityCoords(PlayerPedId())\nlocal x = c.x\n");
        assert!(h.contains(&": vector3@ = ".to_string()) && h.contains(&": number@ = ".to_string()), "{h:?}");
        assert_eq!(super::tidy("nil|unknown".into()), None);
        assert_eq!(super::tidy("<T>[]".into()), None);
        assert_eq!(super::tidy("integer|unknown|nil".into()).as_deref(), Some("integer|nil"));
        assert_eq!(super::tidy("table<string, integer>|any".into()).as_deref(), Some("table<string, integer>"));
        assert_eq!(super::tidy("{ id: string, def: LegalShift, action: LegalAction|nil, total: number }".into()).as_deref(), Some("table"));
        // A function returning itself ends.
        let h = hints("local function f() return f() end\nlocal y = f()\n");
        assert!(h.is_empty(), "{h:?}");
        // Library methods called with `:` do not shift onto `self` (`string.rep(s, n)`).
        let h = hints("local s = 'x'\nlocal r = s:rep(3)\nlocal g = ('x'):gsub('a', 'b')\nlocal q = string.rep(s, 2)\n");
        assert!(!h.iter().any(|n| n.starts_with("s:@3") || n.starts_with("s:@'a'")), "{h:?}");
        assert!(h.iter().any(|n| n.starts_with("n:@2")), "{h:?}");
    }
}
