//! The embedded library: Lua 5.4's standard library (LuaLS meta) and FiveM's
//! natives and runtime (overextended/fivem-lls-addon), generated into
//! `crates/ide/library` by `tools/gen_library.lua`.
//!
//! Parsed once, on first use, into a name index: each global or `A.b` field the
//! library defines, with the game and side it exists on and its doc comment.

mod files;

use crate::{TextRange, TextSize};
use cfx_lua_syntax::SyntaxKind::*;
use cfx_lua_syntax::SyntaxNode;
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    Shared,
    Client,
    Server,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Game {
    /// Lua itself and the CFX runtime.
    Any,
    Gta5,
    Rdr3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Lua,
    /// FiveM runtime globals (`Citizen`, events, `json`, vectors, ...).
    Runtime,
    Native,
}

#[derive(Debug, Clone)]
pub struct LibItem {
    pub file: usize,
    /// The defining name (`Wait`, `Citizen.Wait`) in its file.
    pub range: TextRange,
    /// The comment block above the definition, if any.
    pub doc: Option<TextRange>,
    pub game: Game,
    pub side: Side,
    pub origin: Origin,
    pub function: bool,
}

pub struct Library {
    /// By name: `Wait`, `Citizen.Wait`, `string.format`.
    items: HashMap<String, Vec<LibItem>>,
}

pub fn file_path(file: usize) -> &'static str {
    files::FILES[file].0
}

pub fn file_text(file: usize) -> &'static str {
    files::FILES[file].1
}

/// The library's `---@class` blocks by name (`vector3`, `StateBag`).
pub fn classes() -> &'static HashMap<String, Vec<crate::api::ClassDef>> {
    static CLASSES: OnceLock<HashMap<String, Vec<crate::api::ClassDef>>> = OnceLock::new();
    CLASSES.get_or_init(|| {
        let mut out: HashMap<String, Vec<crate::api::ClassDef>> = HashMap::new();
        for (_, text) in files::FILES {
            if !text.contains("@class") {
                continue;
            }
            for c in crate::api::doc_classes(text) {
                out.entry(c.name.clone()).or_default().push(c);
            }
        }
        out
    })
}

/// The `---@version` of a library global none of whose definitions is for
/// Lua 5.4 (`unpack`: `5.1`); None when one is, or one has no version.
pub fn other_version(name: &str) -> Option<String> {
    let items = library().get(name);
    let mut first = None;
    for item in items {
        let doc = &file_text(item.file)[item.doc?];
        let v = doc.lines().find_map(|l| l.trim().strip_prefix("---@version "))?.trim().to_string();
        if version_has(&v, (5, 4)) {
            return None;
        }
        first.get_or_insert(v);
    }
    first
}

/// Whether a `---@version` list (`5.1`, `>5.2, JIT`, `<5.2`) takes `want`.
fn version_has(list: &str, want: (u32, u32)) -> bool {
    let parse = |s: &str| -> Option<(u32, u32)> {
        let (a, b) = s.trim().split_once('.')?;
        Some((a.parse().ok()?, b.parse().ok()?))
    };
    list.split(',').map(str::trim).any(|p| {
        if let Some(v) = p.strip_prefix('>').and_then(parse) {
            want >= v
        } else if let Some(v) = p.strip_prefix('<').and_then(parse) {
            want <= v
        } else {
            parse(p) == Some(want)
        }
    })
}

/// The members of a library class that a global module table holds
/// (`---@class tablelib` above `table = {}`: `insert`, `concat`, ...); None
/// for any other class.
pub fn module_members(class: &str) -> Option<&'static std::collections::HashSet<String>> {
    static MODULES: OnceLock<HashMap<String, std::collections::HashSet<String>>> = OnceLock::new();
    MODULES
        .get_or_init(|| {
            let mut globals: HashMap<String, String> = HashMap::new();
            for (_, text) in files::FILES {
                let mut class: Option<&str> = None;
                for line in text.lines() {
                    let t = line.trim();
                    if let Some(c) = t.strip_prefix("---@class ") {
                        class = c.split_whitespace().next();
                        continue;
                    }
                    if t.starts_with("---") {
                        continue;
                    }
                    if let (Some(c), Some(g)) = (class.take(), t.strip_suffix("= {}").map(str::trim))
                        && !g.is_empty()
                        && g.chars().all(|ch| ch.is_alphanumeric() || ch == '_')
                    {
                        globals.insert(c.to_string(), g.to_string());
                    }
                }
            }
            let lib = library();
            globals
                .into_iter()
                .map(|(c, g)| {
                    let prefix = format!("{g}.");
                    let mut members: std::collections::HashSet<String> =
                        lib.names().filter_map(|(n, _)| n.strip_prefix(&prefix)).filter(|m| !m.contains('.')).map(str::to_string).collect();
                    for def in classes().get(&c).into_iter().flatten() {
                        members.extend(def.fields.iter().map(|f| f.name.clone()));
                    }
                    (c, members)
                })
                .collect()
        })
        .get(class)
}

/// Every `---@class`, `---@alias` and `---@enum` name of the library.
pub fn doc_names() -> &'static std::collections::HashSet<String> {
    static NAMES: OnceLock<std::collections::HashSet<String>> = OnceLock::new();
    NAMES.get_or_init(|| {
        let mut out: std::collections::HashSet<String> = classes().keys().cloned().collect();
        for (_, text) in files::FILES {
            if text.contains("@alias") || text.contains("@enum") {
                out.extend(crate::api::doc_type_names(text));
            }
        }
        out
    })
}

/// The `---@class` a table global of the library is declared as (`utf8lib`).
pub fn global_class(global: &str) -> Option<String> {
    let item = library().get(global).iter().find(|i| !i.function && i.doc.is_some())?;
    let (tags, _) = crate::luacats::parse_block(&file_text(item.file)[item.doc?], TextSize::from(0));
    tags.into_iter().find_map(|t| match t.tag {
        crate::luacats::Tag::Class { name, .. } => Some(name),
        _ => None,
    })
}

/// Every `---@alias` of the library by name.
pub fn aliases() -> &'static std::collections::HashMap<String, crate::luacats::DocType> {
    static ALIASES: OnceLock<std::collections::HashMap<String, crate::luacats::DocType>> = OnceLock::new();
    ALIASES.get_or_init(|| files::FILES.iter().filter(|(_, t)| t.contains("@alias")).flat_map(|(_, t)| crate::types::doc_aliases(t).0).collect())
}

/// The `---@return` types of a library function.
pub fn returns(item: &LibItem) -> Vec<crate::luacats::DocType> {
    let Some(r) = item.doc else { return Vec::new() };
    let (tags, _) = crate::luacats::parse_block(&file_text(item.file)[r], TextSize::from(0));
    tags.into_iter()
        .filter_map(|t| match t.tag {
            crate::luacats::Tag::Return { rets, .. } => Some(rets),
            _ => None,
        })
        .flatten()
        .map(|r| r.ty)
        .collect()
}

pub fn library() -> &'static Library {
    static LIB: OnceLock<Library> = OnceLock::new();
    LIB.get_or_init(build)
}

/// On plain scoped threads, not rayon: callers are often workers of the global pool
/// (lint in `check`) while the other workers block on the `OnceLock`. A build on
/// the global pool would wait for those blocked workers, and one on another pool
/// lets the waiting worker steal a job that calls `library()` again and re-enters
/// the `OnceLock`.
fn build() -> Library {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(16);
    let chunk = files::FILES.len().div_ceil(threads).max(1);
    let per_file: Vec<Vec<(String, LibItem)>> = std::thread::scope(|s| {
        let handles: Vec<_> = files::FILES
            .chunks(chunk)
            .enumerate()
            .map(|(c, part)| {
                s.spawn(move || {
                    part.iter()
                        .enumerate()
                        .flat_map(|(i, (path, text))| index_file(c * chunk + i, path, text))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap_or_else(|e| std::panic::resume_unwind(e))).collect()
    });
    let mut items: HashMap<String, Vec<LibItem>> = HashMap::new();
    for (name, item) in per_file.into_iter().flatten() {
        items.entry(name).or_default().push(item);
    }
    Library { items }
}

impl Library {
    pub fn get(&self, name: &str) -> &[LibItem] {
        self.items.get(name).map_or(&[], Vec::as_slice)
    }

    /// Whether `name` is a global of the library for that game.
    pub fn has_global(&self, name: &str, game: Game) -> bool {
        self.get(name).iter().any(|i| i.game == Game::Any || i.game == game)
    }

    /// The names the library defines, for completion.
    pub fn names(&self) -> impl Iterator<Item = (&str, &[LibItem])> {
        self.items.iter().map(|(k, v)| (k.as_str(), v.as_slice()))
    }

    /// The doc comment text of an item, without the `---` markers.
    pub fn doc(&self, item: &LibItem) -> String {
        let Some(r) = item.doc else { return String::new() };
        file_text(item.file)[r].lines().map(|l| l.trim_start().trim_start_matches('-')).collect::<Vec<_>>().join("\n")
    }

    /// `function GetEntityCoords(entity, alive)` for a function, the name otherwise.
    pub fn declaration(&self, item: &LibItem) -> String {
        let text = file_text(item.file);
        let line = text[usize::from(item.range.start())..].lines().next().unwrap_or_default().trim();
        if item.function {
            format!("function {}", line.trim_end_matches("end").trim_end())
        } else {
            text[item.range].to_string()
        }
    }

    /// A function's parameters with the type of their `---@param` line.
    pub fn params(&self, item: &LibItem) -> Vec<(String, Option<String>)> {
        let decl = self.declaration(item);
        let Some(list) = decl.split_once('(').and_then(|(_, rest)| rest.rsplit_once(')')).map(|(l, _)| l) else {
            return Vec::new();
        };
        let doc = self.doc(item);
        list.split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(|p| {
                let ty = doc.lines().find_map(|l| {
                    let rest = l.trim().strip_prefix("@param")?.trim_start();
                    let (name, ty) = rest.split_once(char::is_whitespace)?;
                    (name.trim_end_matches('?') == p).then(|| ty.split_whitespace().next().unwrap_or_default().to_string())
                });
                (p.to_string(), ty)
            })
            .collect()
    }
}

fn index_file(file: usize, path: &str, text: &str) -> Vec<(String, LibItem)> {
    let (game, origin) = if path.starts_with("lua54/") {
        (Game::Any, Origin::Lua)
    } else if path.starts_with("fivem/runtime/") {
        (Game::Any, Origin::Runtime)
    } else if path.starts_with("fivem/natives/GTAV/") {
        (Game::Gta5, Origin::Native)
    } else if path.starts_with("fivem/natives/RDR3/") {
        (Game::Rdr3, Origin::Native)
    } else {
        (Game::Any, Origin::Native)
    };
    let parse = cfx_lua_syntax::parse(text);
    let root = parse.syntax();
    let Some(block) = root.children().find(|c| c.kind() == BLOCK) else { return Vec::new() };
    let mut out = Vec::new();
    for stat in block.children() {
        let (names, function): (Vec<(String, TextRange)>, bool) = match stat.kind() {
            FUNCTION_STAT => {
                let Some(name) = stat.children().find(|c| c.kind() == FUNC_NAME) else { continue };
                let text_name: String = name
                    .children_with_tokens()
                    .filter_map(|e| e.into_token())
                    .filter(|t| matches!(t.kind(), NAME | DOT | COLON))
                    .map(|t| if t.kind() == COLON { ".".to_string() } else { t.text().to_string() })
                    .collect();
                (vec![(text_name, name.text_range())], true)
            }
            ASSIGN_STAT => {
                let Some(vars) = stat.children().find(|c| c.kind() == VAR_LIST) else { continue };
                let function = stat
                    .children()
                    .find(|c| c.kind() == EXPR_LIST)
                    .and_then(|l| l.children().next())
                    .is_some_and(|e| e.kind() == FUNCTION_EXPR || is_function_alias(&e, text));
                (vars.children().filter_map(|v| dotted(&v).map(|n| (n, v.text_range()))).collect(), function)
            }
            _ => continue,
        };
        let doc = doc_range(&stat);
        let side = match (origin, doc.map(|r| &text[r])) {
            (Origin::Native, Some(d)) => native_side(d),
            _ => Side::Shared,
        };
        for (name, range) in names {
            out.push((name, LibItem { file, range, doc, game, side, origin, function }));
        }
    }
    out
}

/// `CreateThread = Citizen.CreateThread`: a function under another name.
fn is_function_alias(expr: &SyntaxNode, _text: &str) -> bool {
    matches!(expr.kind(), INDEX_EXPR | NAME_EXPR)
}

/// `a`, `a.b`, `a.b.c` for a name or a chain of `.name` fields.
fn dotted(expr: &SyntaxNode) -> Option<String> {
    match expr.kind() {
        NAME_EXPR => expr.first_token().map(|t| t.text().to_string()),
        INDEX_EXPR => {
            let object = dotted(&expr.first_child()?)?;
            let has_dot = expr.children_with_tokens().any(|e| e.kind() == DOT);
            let field = expr.children_with_tokens().filter_map(|e| e.into_token()).filter(|t| t.kind() == NAME).last()?;
            has_dot.then(|| format!("{object}.{}", field.text()))
        }
        _ => None,
    }
}

/// The comment lines at the start of a statement (its doc block).
fn doc_range(stat: &SyntaxNode) -> Option<TextRange> {
    let start = stat.text_range().start();
    let mut end: Option<TextSize> = None;
    for t in stat.children_with_tokens().filter_map(|e| e.into_token()) {
        match t.kind() {
            COMMENT | BLOCK_COMMENT => end = Some(t.text_range().end()),
            WHITESPACE => {}
            _ => break,
        }
    }
    end.map(|e| TextRange::new(start, e))
}

/// `---**`APP` `client`**`: the side in the native's first doc line. RDR3 natives
/// carry none and are client natives.
fn native_side(doc: &str) -> Side {
    let first = doc.lines().next().unwrap_or("");
    if first.contains("`server`") {
        Side::Server
    } else if first.contains("`shared`") {
        Side::Shared
    } else {
        Side::Client
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_names() {
        let lib = library();
        let wait = lib.get("Wait");
        assert!(wait.iter().any(|i| i.origin == Origin::Runtime && i.function), "{wait:?}");
        assert!(lib.get("Citizen.CreateThread")[0].function);
        assert!(lib.has_global("print", Game::Gta5));
        assert!(lib.has_global("string.format", Game::Gta5));
        let ped = lib.get("PlayerPedId");
        assert!(ped.iter().any(|i| i.game == Game::Gta5 && i.side == Side::Client));
        assert!(lib.get("GetPlayerName").iter().any(|i| i.side == Side::Server), "{:?}", lib.get("GetPlayerName"));
        assert!(lib.doc(&ped[0]).contains("Native Documentation"));
        assert!(!lib.has_global("NotARealNative", Game::Gta5));
    }
}
