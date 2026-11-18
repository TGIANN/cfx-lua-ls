//! `cfx-lua-ls mcp`: a Model Context Protocol server on stdio for AI agents.
//! One JSON-RPC message per line in each direction (MCP 2025-06-18 stdio
//! transport); logs go to stderr. Tools: `check` (diagnostics for a file or
//! folder, optionally of unsaved text, with fixes), `lint_snippet` (code that is
//! not on disk yet), `native` (natives and library functions: side, game, docs),
//! `rules`, `resource_info` (manifest sides and imports), and over a folder of
//! resources: `exports` (a resource's exports), `members` (what an expression
//! such as `QBCore.Functions` or `lib` holds), `events` (who registers and
//! triggers an event) and `symbols` (where a global, field, export or event
//! handler is defined). A scanned folder stays in memory between calls and is
//! brought up to date from the disk's sizes and modification times.

use crate::check::{self, Found};
use cfx_lua_ide::library::{Game, Origin, Side, library};
use cfx_lua_ide::lint::{Context, LintConfig, RULES, Severity, describe};
use cfx_lua_ide::manifest::FileSide;
use line_index::LineIndex;
use rayon::prelude::*;
use serde_json::{Value, json};
use cfx_lua_ide::workspace::{Stamp, Workspace};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// A folder scanned by an earlier call.
struct Scanned {
    folder: PathBuf,
    stamp: Stamp,
    ws: Workspace,
}

/// The folders scanned last (two at most: a server's resources and one more).
static SCANNED: Mutex<Vec<Scanned>> = Mutex::new(Vec::new());
const MAX_SCANNED: usize = 2;

const VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
const LATEST: &str = "2025-06-18";

const INSTRUCTIONS: &str = "Diagnostics and API facts for FiveM Lua (CfxLua, Lua 5.4). Run `check` on files you changed and fix what it reports; run `lint_snippet` on code before writing it to a file; look up a native with `native` before using it (it tells whether it is client, server or shared). Before calling another resource, ask `exports` for its exports, `members` for what a framework object holds (`QBCore.Functions`, `lib`), and `events` for who handles an event: the answers come from the server's own copies of the resources. The side of a file comes from its resource's fxmanifest.lua.";

pub(crate) fn run_stdio() {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = handle(&line) {
            // `Value`'s Display never writes a raw newline: one message per line.
            if writeln!(out, "{response}").and_then(|_| out.flush()).is_err() {
                break;
            }
        }
    }
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// The response to one message; `None` for notifications and responses.
pub(crate) fn handle(line: &str) -> Option<Value> {
    let Ok(msg) = serde_json::from_str::<Value>(line) else {
        return Some(error(Value::Null, -32700, "parse error"));
    };
    let id = msg.get("id").cloned()?;
    let Some(method) = msg.get("method").and_then(Value::as_str) else {
        // A response to a request we never sent.
        return None;
    };
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => {
            let asked = params["protocolVersion"].as_str().unwrap_or(LATEST);
            let version = if VERSIONS.contains(&asked) { asked } else { LATEST };
            json!({
                "protocolVersion": version,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "cfx-lua-ls", "version": env!("CARGO_PKG_VERSION") },
                "instructions": INSTRUCTIONS,
            })
        }
        "ping" => json!({}),
        "tools/list" => json!({ "tools": tools() }),
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or_default();
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            let outcome = match name {
                "check" => tool_check(&args),
                "lint_snippet" => tool_snippet(&args),
                "native" => tool_native(&args),
                "rules" => tool_rules(&args),
                "resource_info" => tool_resource(&args),
                "exports" => tool_exports(&args),
                "members" => tool_members(&args),
                "events" => tool_events(&args),
                "symbols" => tool_symbols(&args),
                _ => return Some(error(id, -32602, &format!("unknown tool: {name}"))),
            };
            match outcome {
                Ok(v) => json!({
                    "content": [{ "type": "text", "text": v.to_string() }],
                    "structuredContent": v,
                    "isError": false,
                }),
                Err(e) => json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
            }
        }
        _ => return Some(error(id, -32601, &format!("method not found: {method}"))),
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

fn tools() -> Value {
    let level = json!({
        "type": "string",
        "enum": ["error", "warning", "information", "hint"],
        "description": "Lowest severity to report (default: warning)."
    });
    json!([
        {
            "name": "check",
            "title": "Check FiveM Lua",
            "description": "Syntax errors and lint diagnostics (LuaLS rules, FiveM side / sandbox / performance rules, ox_lib suggestions) for a .lua file or a folder of resources. The resource's fxmanifest.lua decides each file's side and imports. Pass `text` to check unsaved content of `path`. Diagnostics with a quick fix carry its edits; `fix: true` (one file) also returns the text with every fix applied.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Absolute path of a .lua file or a folder." },
                    "text": { "type": "string", "description": "Content to check instead of the file on disk (path must be a file)." },
                    "level": level,
                    "limit": { "type": "integer", "description": "Most diagnostics to return (default 100); counts cover all." },
                    "fix": { "type": "boolean", "description": "Return `fixedText`: the file with all non-overlapping quick fixes applied (not written)." }
                },
                "required": ["path"]
            },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "lint_snippet",
            "title": "Lint a FiveM Lua snippet",
            "description": "Diagnostics for Lua code that is not in a file yet, as if it were a client, server or shared script. Globals defined by other files are unknown here: list them in `globals`. Returns `fixedText` when quick fixes apply.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "code": { "type": "string" },
                    "side": { "type": "string", "enum": ["client", "server", "shared"], "description": "Default: client." },
                    "ox_lib": { "type": "boolean", "description": "The manifest loads @ox_lib/init.lua (enables ox_lib suggestions and its globals)." },
                    "globals": { "type": "array", "items": { "type": "string" }, "description": "Globals defined elsewhere (Config, QBCore, ...)." },
                    "level": level
                },
                "required": ["code"]
            },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "native",
            "title": "Look up a native or library function",
            "description": "FiveM natives (GTA V, RDR3, CFX), FiveM runtime functions and the Lua 5.4 library. `name` returns the exact entry: side (client / server / shared), game and its documentation with parameters. `query` searches names (case-insensitive substring).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Exact name, e.g. GetEntityCoords or string.format." },
                    "query": { "type": "string", "description": "Part of a name." },
                    "limit": { "type": "integer", "description": "Most search results (default 20)." }
                }
            },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "rules",
            "title": "Diagnostic rules",
            "description": "Every rule code with its default severity and what it flags; `code` for one. Rules are turned off or re-levelled in .luarc.json (`diagnostics.disable`, `diagnostics.severity`) or per line with `---@diagnostic disable-next-line: <code>`.",
            "inputSchema": { "type": "object", "properties": { "code": { "type": "string" } } },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "exports",
            "title": "A resource's exports",
            "description": "The exports another resource can call on `resource` (also a name it answers through `provide`, such as qb-core for qbx_core), read from the resources under `folder`: name, parameters, the side that registers it, `---@deprecated` advice, doc comment and defining file:line. `complete: false` means some exports cannot be known (escrow, computed names, JavaScript or .NET scripts).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "folder": { "type": "string", "description": "Absolute path of the server's resources folder (or any folder holding the resources)." },
                    "resource": { "type": "string" },
                    "side": { "type": "string", "enum": ["client", "server"], "description": "The caller's side; exports of the other side are listed apart." }
                },
                "required": ["folder", "resource"]
            },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "members",
            "title": "Members of a Lua expression",
            "description": "What a value holds, as the editor's completion sees it at the end of `path`: `QBCore.Functions`, `QBCore` (a local set from `exports['qb-core']:GetCoreObject()`), `lib` or `lib.callback` with ox_lib, `exports.qbx_core:GetCoreObject().Functions`. Each member: name, parameters, deprecation, doc, defining file:line. Resolved through the resources under `folder`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "folder": { "type": "string", "description": "Absolute path of the server's resources folder." },
                    "path": { "type": "string", "description": "Absolute path of the .lua file the expression is written in (its locals, side and imports apply)." },
                    "text": { "type": "string", "description": "Unsaved content of `path`." },
                    "expression": { "type": "string" }
                },
                "required": ["folder", "path", "expression"]
            },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "events",
            "title": "Event handlers and triggers",
            "description": "Where an event is registered (RegisterNetEvent / AddEventHandler) and triggered (TriggerEvent, TriggerServerEvent, TriggerClientEvent) across the resources under `folder`. `name` for one event, `query` to search names.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "folder": { "type": "string" },
                    "name": { "type": "string" },
                    "query": { "type": "string", "description": "Part of an event name (case-insensitive)." },
                    "limit": { "type": "integer", "description": "Most results (default 50)." }
                },
                "required": ["folder"]
            },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "symbols",
            "title": "Find definitions",
            "description": "Where globals, their fields (`QBCore.Functions.GetPlayer`), exports and event handlers are defined across the resources under `folder`, by name (fuzzy: the query's letters in order). Each hit: name, container, kind, file:line.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "folder": { "type": "string" },
                    "query": { "type": "string" },
                    "limit": { "type": "integer", "description": "Most results (default 50)." }
                },
                "required": ["folder", "query"]
            },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "resource_info",
            "title": "Resource and file side",
            "description": "The resource a file belongs to: its fxmanifest.lua lists (client, server, shared, files, dependencies), its exports with the side that registers each, the file's side, whether the manifest loads it, whether ox_lib is loaded for it and the globals the file defines.",
            "inputSchema": {
                "type": "object",
                "properties": { "path": { "type": "string", "description": "Absolute path of a file in the resource, or the resource folder." } },
                "required": ["path"]
            },
            "annotations": { "readOnlyHint": true }
        }
    ])
}

fn level(args: &Value, default: Severity) -> Result<Severity, String> {
    match args["level"].as_str() {
        None => Ok(default),
        Some(s) => Severity::parse(s).ok_or_else(|| format!("unknown level `{s}`")),
    }
}

fn side_name(s: FileSide) -> &'static str {
    match s {
        FileSide::Client => "client",
        FileSide::Server => "server",
        FileSide::Shared => "shared",
        FileSide::Unknown => "unknown",
    }
}

/// Diagnostics most severe first, capped, with counts over all of them.
fn report(mut found: Vec<Found>, base: &Path, limit: usize, files: usize) -> Value {
    found.sort_by(|a, b| (a.severity, &a.file, a.range).cmp(&(b.severity, &b.file, b.range)));
    let mut by_code = serde_json::Map::new();
    for f in &found {
        let n = by_code.get(&f.code).and_then(Value::as_u64).unwrap_or(0);
        by_code.insert(f.code.clone(), json!(n + 1));
    }
    let mut by_severity = serde_json::Map::new();
    for s in [Severity::Error, Severity::Warning, Severity::Information, Severity::Hint] {
        by_severity.insert(check::severity_name(s).into(), json!(found.iter().filter(|f| f.severity == s).count()));
    }
    let list: Vec<Value> = found.iter().take(limit).map(|f| check::to_json(f, base)).collect();
    json!({
        "diagnostics": list,
        "summary": {
            "files": files,
            "total": found.len(),
            "truncated": found.len() > limit,
            "bySeverity": by_severity,
            "byCode": by_code,
        }
    })
}

fn tool_check(args: &Value) -> Result<Value, String> {
    let path = PathBuf::from(args["path"].as_str().ok_or("`path` is required")?);
    let text = args["text"].as_str();
    if text.is_none() && !path.exists() {
        return Err(format!("{} does not exist", path.display()));
    }
    if text.is_some() && path.is_dir() {
        return Err("`text` needs `path` to be a file".into());
    }
    let min = level(args, Severity::Warning)?;
    let limit = args["limit"].as_u64().unwrap_or(100) as usize;
    let config = crate::config::load_for(&path);
    let (mut ws, mut files) = if path.is_dir() {
        check::load(std::slice::from_ref(&path))
    } else {
        let mut ws = cfx_lua_ide::workspace::Workspace::default();
        ws.ensure(&path);
        (ws, vec![path.clone()])
    };
    if path.is_dir() {
        files.retain(|f| ws.info(f).loaded);
    }
    if let Some(t) = text {
        ws.update(&path, t);
    }
    let read = |f: &Path| -> String {
        match text {
            Some(t) if f == path => t.to_string(),
            _ => std::fs::read(f).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default(),
        }
    };
    let found: Vec<Found> = files
        .par_iter()
        .flat_map_iter(|f| check::diagnose(&ws, f, &read(f), &config))
        .filter(|f| f.severity <= min)
        .collect();
    let base = if path.is_dir() { path.clone() } else { path.parent().map(Path::to_path_buf).unwrap_or_default() };
    let mut out = report(found, &base, limit, files.len());
    if args["fix"].as_bool() == Some(true) && path.is_file() {
        let source = read(&path);
        let fixed = check::fix_text(&ws, &path, &source, &config, min);
        out["fixedText"] = fixed.as_ref().map_or(Value::Null, |(t, _)| json!(t));
        out["fixesApplied"] = json!(fixed.map_or(0, |(_, n)| n));
    }
    Ok(out)
}

fn tool_snippet(args: &Value) -> Result<Value, String> {
    let code = args["code"].as_str().ok_or("`code` is required")?;
    let side = match args["side"].as_str().unwrap_or("client") {
        "client" => FileSide::Client,
        "server" => FileSide::Server,
        "shared" => FileSide::Shared,
        s => return Err(format!("unknown side `{s}`")),
    };
    let min = level(args, Severity::Hint)?;
    let mut config = LintConfig::default();
    if let Some(list) = args["globals"].as_array() {
        config.globals.extend(list.iter().filter_map(|g| g.as_str().map(str::to_string)));
    }
    let ox = args["ox_lib"].as_bool().unwrap_or(false);
    if ox {
        config.globals.extend(["locale", "SetInterval", "ClearInterval"].map(String::from));
    }
    let parse = cfx_lua_ide::parse(code);
    let names = cfx_lua_ide::names(&parse, code);
    let ctx = Context { side, ox_lib: ox, config: &config, ..Default::default() };
    let lints = cfx_lua_ide::lint::lint(&parse, code, &names, &ctx);
    let index = LineIndex::new(code);
    let file = PathBuf::from("snippet.lua");
    let mut found: Vec<Found> = cfx_lua_ide::syntax_diagnostics(&parse)
        .into_iter()
        .map(|d| Found {
            file: file.clone(),
            range: check::span(&index, code, d.range),
            severity: Severity::Error,
            code: d.code,
            message: d.message,
            fix: None,
        })
        .collect();
    found.extend(lints.iter().filter(|l| l.severity <= min).map(|l| check::found(&file, &index, code, l)));
    let mut out = report(found, Path::new(""), 500, 1);
    if parse.errors().is_empty() {
        let fixable: Vec<_> = lints.into_iter().filter(|l| l.severity <= min).collect();
        let (text, n) = cfx_lua_ide::lint::apply_fixes(code, &fixable);
        if n > 0 {
            out["fixedText"] = json!(text);
            out["fixesApplied"] = json!(n);
        }
    }
    Ok(out)
}

fn item_json(name: &str, item: &cfx_lua_ide::library::LibItem, doc: bool) -> Value {
    let side = match (item.origin, item.side) {
        (Origin::Native, Side::Client) => "client",
        (Origin::Native, Side::Server) => "server",
        _ => "shared",
    };
    let game = match item.game {
        Game::Gta5 => "gta5",
        Game::Rdr3 => "rdr3",
        Game::Any => "any",
    };
    let origin = match item.origin {
        Origin::Lua => "lua",
        Origin::Runtime => "fivem-runtime",
        Origin::Native => "native",
    };
    let mut v = json!({ "name": name, "side": side, "game": game, "kind": origin, "function": item.function });
    if doc {
        v["doc"] = json!(library().doc(item));
        let text = cfx_lua_ide::library::file_text(item.file);
        // The declaration line (`function GetEntityCoords(entity, alive) end`).
        let decl = text[item.range.start().into()..].lines().next().unwrap_or_default();
        v["declaration"] = json!(if item.function { format!("function {}", decl.trim()) } else { decl.trim().to_string() });
    }
    v
}

fn tool_native(args: &Value) -> Result<Value, String> {
    let lib = library();
    if let Some(name) = args["name"].as_str() {
        let mut items: Vec<Value> = lib.get(name).iter().map(|i| item_json(name, i, true)).collect();
        if items.is_empty() {
            // Case-insensitive exact match.
            if let Some((n, list)) = lib.names().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
                items = list.iter().map(|i| item_json(n, i, true)).collect();
            }
        }
        if !items.is_empty() {
            return Ok(json!({ "entries": items }));
        }
        if args["query"].is_null() {
            return Err(format!("no native or library function named `{name}`; try `query`"));
        }
    }
    let query = args["query"].as_str().ok_or("`name` or `query` is required")?.to_lowercase();
    let limit = args["limit"].as_u64().unwrap_or(20) as usize;
    let mut hits: Vec<(bool, &str, &cfx_lua_ide::library::LibItem)> = lib
        .names()
        .filter_map(|(n, list)| {
            let lower = n.to_lowercase();
            lower.contains(&query).then(|| (!lower.starts_with(&query), n, &list[0]))
        })
        .collect();
    hits.sort_by(|a, b| (a.0, a.1.len(), a.1).cmp(&(b.0, b.1.len(), b.1)));
    let total = hits.len();
    let list: Vec<Value> = hits.into_iter().take(limit).map(|(_, n, i)| item_json(n, i, false)).collect();
    Ok(json!({ "matches": list, "total": total }))
}

fn tool_rules(args: &Value) -> Result<Value, String> {
    let entry = |code: &str, s: Option<Severity>| {
        json!({
            "code": code,
            "defaultSeverity": s.map_or("off", check::severity_name),
            "description": describe(code).unwrap_or_default(),
        })
    };
    match args["code"].as_str() {
        Some(code) => RULES
            .iter()
            .find(|(c, _)| *c == code)
            .map(|(c, s)| entry(c, *s))
            .ok_or_else(|| format!("no rule `{code}`")),
        None => Ok(json!({ "rules": RULES.iter().map(|(c, s)| entry(c, *s)).collect::<Vec<_>>() })),
    }
}

/// Runs `f` over the workspace of `args.folder`: the one kept from an earlier
/// call, refreshed from the disk, or a new scan.
fn with_folder<T>(args: &Value, f: impl FnOnce(&Path, &mut Workspace) -> Result<T, String>) -> Result<T, String> {
    let folder = PathBuf::from(args["folder"].as_str().ok_or("`folder` is required")?);
    if !folder.is_dir() {
        return Err(format!("{} is not a folder", folder.display()));
    }
    let stamp = cfx_lua_ide::workspace::stamp(std::slice::from_ref(&folder));
    let mut scanned = SCANNED.lock().unwrap_or_else(|e| e.into_inner());
    let i = match scanned.iter().position(|s| s.folder == folder) {
        Some(i) => {
            let s = &mut scanned[i];
            if s.stamp != stamp {
                s.ws.refresh(&s.stamp, &stamp);
                s.stamp = stamp;
            }
            i
        }
        None => {
            if scanned.len() >= MAX_SCANNED {
                scanned.remove(0);
            }
            let ws = Workspace::scan(std::slice::from_ref(&folder));
            scanned.push(Scanned { folder: folder.clone(), stamp, ws });
            scanned.len() - 1
        }
    };
    f(&folder, &mut scanned[i].ws)
}

/// `file:line` relative to `base`, 1-based.
fn place(base: &Path, file: &Path, offset: u32) -> String {
    let line = std::fs::read(file)
        .map(|b| b.iter().take(offset as usize).filter(|c| **c == b'\n').count() + 1)
        .unwrap_or(0);
    let rel = file.strip_prefix(base).unwrap_or(file).to_string_lossy().replace('\\', "/");
    format!("{rel}:{line}")
}

fn api_item_json(base: &Path, i: &cfx_lua_ide::workspace::ApiItem) -> Value {
    let mut v = json!({ "name": i.name, "signature": i.signature() });
    if let Some(d) = i.deprecated() {
        v["deprecated"] = json!(d);
    }
    if !i.doc.is_empty() {
        v["doc"] = json!(i.doc);
    }
    if let Some((f, o)) = &i.location {
        v["defined"] = json!(place(base, f, *o));
    }
    if i.side != FileSide::Unknown {
        v["side"] = json!(side_name(i.side));
    }
    v
}

fn tool_exports(args: &Value) -> Result<Value, String> {
    with_folder(args, |folder, ws| exports_in(args, folder, ws))
}

fn exports_in(args: &Value, folder: &Path, ws: &mut Workspace) -> Result<Value, String> {
    let resource = args["resource"].as_str().ok_or("`resource` is required")?;
    let side = match args["side"].as_str() {
        Some("client") => FileSide::Client,
        Some("server") => FileSide::Server,
        _ => FileSide::Unknown,
    };
    let e = ws.exports_on(resource, side).ok_or_else(|| format!("no resource under {} is or provides `{resource}`", folder.display()))?;
    let mut out = json!({
        "resource": e.resource,
        "complete": e.complete,
        "exports": e.items.iter().map(|i| api_item_json(folder, i)).collect::<Vec<_>>(),
    });
    if !e.other_side.is_empty() {
        out["otherSide"] = json!(e.other_side.iter().map(|i| api_item_json(folder, i)).collect::<Vec<_>>());
    }
    Ok(out)
}

fn tool_members(args: &Value) -> Result<Value, String> {
    with_folder(args, |folder, ws| members_in(args, folder, ws))
}

fn members_in(args: &Value, folder: &Path, ws: &mut Workspace) -> Result<Value, String> {
    let path = PathBuf::from(args["path"].as_str().ok_or("`path` is required")?);
    let expression = args["expression"].as_str().ok_or("`expression` is required")?;
    let text = match args["text"].as_str() {
        Some(t) => t.to_string(),
        None => String::from_utf8_lossy(&std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?).into_owned(),
    };
    ws.ensure(&path);
    let items = ws
        .members_of_expression(&path, &text, expression)
        .ok_or_else(|| format!("`{expression}` resolves to nothing known at the end of {}", path.display()))?;
    Ok(json!({ "expression": expression, "members": items.iter().map(|i| api_item_json(folder, i)).collect::<Vec<_>>() }))
}

fn tool_events(args: &Value) -> Result<Value, String> {
    with_folder(args, |folder, ws| events_in(args, folder, ws))
}

fn events_in(args: &Value, folder: &Path, ws: &mut Workspace) -> Result<Value, String> {
    let limit = args["limit"].as_u64().unwrap_or(50) as usize;
    if let Some(name) = args["name"].as_str() {
        let sites = ws.event_sites(name);
        if sites.is_empty() {
            return Err(format!("no file under {} registers or triggers `{name}`", folder.display()));
        }
        let list = |handled: bool| -> Vec<String> {
            sites.iter().filter(|s| s.2 == handled).take(limit).map(|(f, o, _)| place(folder, f, *o)).collect()
        };
        return Ok(json!({ "event": name, "registered": list(true), "triggered": list(false) }));
    }
    let query = args["query"].as_str().ok_or("`name` or `query` is required")?.to_lowercase();
    let hits: Vec<Value> = ws
        .event_names()
        .into_iter()
        .filter(|(n, _, _)| n.to_lowercase().contains(&query))
        .map(|(n, h, t)| json!({ "event": n, "handlers": h, "triggers": t }))
        .collect();
    let total = hits.len();
    Ok(json!({ "matches": hits.into_iter().take(limit).collect::<Vec<_>>(), "total": total }))
}

fn tool_symbols(args: &Value) -> Result<Value, String> {
    with_folder(args, |folder, ws| {
        let query = args["query"].as_str().ok_or("`query` is required")?;
        let limit = args["limit"].as_u64().unwrap_or(50) as usize;
        let hits: Vec<Value> = ws
            .symbols(query, limit)
            .iter()
            .map(|s| {
                let kind = match s.kind {
                    cfx_lua_ide::workspace::WsSymbolKind::Function => "function",
                    cfx_lua_ide::workspace::WsSymbolKind::Variable => "variable",
                    cfx_lua_ide::workspace::WsSymbolKind::Field => "field",
                    cfx_lua_ide::workspace::WsSymbolKind::Export => "export",
                    cfx_lua_ide::workspace::WsSymbolKind::Event => "event",
                };
                json!({ "name": s.name, "container": s.container, "kind": kind, "defined": place(folder, &s.path, s.offset) })
            })
            .collect();
        Ok(json!({ "query": query, "symbols": hits }))
    })
}

fn tool_resource(args: &Value) -> Result<Value, String> {
    let given = PathBuf::from(args["path"].as_str().ok_or("`path` is required")?);
    let file = if given.is_dir() { given.join("fxmanifest.lua") } else { given.clone() };
    let mut ws = cfx_lua_ide::workspace::Workspace::default();
    ws.ensure(&file);
    let Some((root, name, m)) = ws.resource(&file) else {
        return Err(format!("{} is not inside a resource (no fxmanifest.lua above it)", given.display()));
    };
    let mut out = json!({
        "resource": name,
        "root": root.to_string_lossy(),
        "manifest": {
            "client": m.client, "server": m.server, "shared": m.shared, "files": m.files,
            "dependencies": m.dependencies, "games": m.games, "dynamic": m.dynamic,
        },
    });
    let name = name.to_string();
    if let Some(e) = ws.exports_on(&name, FileSide::Unknown) {
        let list: Vec<Value> = e
            .items
            .iter()
            .map(|i| json!({ "name": i.name, "signature": i.signature(), "side": side_name(i.side) }))
            .collect();
        out["exports"] = json!(list);
        out["exportsComplete"] = json!(e.complete);
    }
    if given.is_file() {
        let info = ws.info(&given);
        out["file"] = json!({
            "side": side_name(info.side),
            "loaded": info.loaded,
            "oxLib": info.ox_lib,
            "globalsUnknown": info.globals_unknown,
            "globalsDefined": ws.globals_of(&given),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(tool: &str, args: Value) -> Value {
        let line = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": tool, "arguments": args } });
        handle(&line.to_string()).unwrap()["result"].clone()
    }

    #[test]
    fn protocol() {
        let init = handle(r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#).unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
        assert!(init["result"]["capabilities"]["tools"].is_object());
        // Unknown versions get ours; notifications get no answer.
        let init = handle(r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#).unwrap();
        assert_eq!(init["result"]["protocolVersion"], LATEST);
        assert!(handle(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
        let list = handle(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#).unwrap();
        assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 9);
        assert_eq!(handle("{nope").unwrap()["error"]["code"], -32700);
        assert_eq!(handle(r#"{"jsonrpc":"2.0","id":3,"method":"x"}"#).unwrap()["error"]["code"], -32601);
        let bad = handle(r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"nope"}}"#).unwrap();
        assert_eq!(bad["error"]["code"], -32602);
        // A failing tool is a result with isError.
        let r = call("check", json!({ "path": "Z:/does/not/exist.lua" }));
        assert_eq!(r["isError"], true);
    }

    #[test]
    fn snippet_native_rules() {
        let r = call("lint_snippet", json!({ "code": "local ped = PlayerPedId()\nprint(ped, Config)\n", "ox_lib": true, "globals": ["Config"] }));
        let s = &r["structuredContent"];
        assert_eq!(s["diagnostics"][0]["code"], "ox/cache-ped", "{s}");
        assert_eq!(s["fixedText"], "local ped = cache.ped\nprint(ped, Config)\n");
        let r = call("lint_snippet", json!({ "code": "local p = PlayerPedId()\nprint(p)\n", "side": "server" }));
        assert_eq!(r["structuredContent"]["diagnostics"][0]["code"], "fivem/wrong-side-native");
        let r = call("native", json!({ "name": "GetEntityCoords" }));
        // A GTA V client native and a CFX server one share the name.
        let entries = r["structuredContent"]["entries"].as_array().unwrap().clone();
        let sides: Vec<(&str, &str)> = entries.iter().map(|e| (e["side"].as_str().unwrap(), e["game"].as_str().unwrap())).collect();
        assert!(sides.contains(&("client", "gta5")) && sides.contains(&("server", "any")), "{sides:?}");
        assert!(entries[0]["declaration"].as_str().unwrap().starts_with("function GetEntityCoords("));
        let r = call("native", json!({ "query": "entitycoords", "limit": 3 }));
        assert_eq!(r["structuredContent"]["matches"].as_array().unwrap().len(), 3);
        let r = call("rules", json!({ "code": "fivem/source-after-wait" }));
        assert_eq!(r["structuredContent"]["defaultSeverity"], "warning");
    }

    #[test]
    fn check_and_resource_info() {
        let root = std::env::temp_dir().join(format!("cfx-mcp-{}", std::process::id()));
        let res = root.join("app");
        std::fs::create_dir_all(&res).unwrap();
        std::fs::write(res.join("fxmanifest.lua"), "client_script 'c.lua'\nserver_script 's.lua'\n").unwrap();
        std::fs::write(res.join("c.lua"), "Shared = 1\n").unwrap();
        std::fs::write(res.join("s.lua"), "print(Shared)\n").unwrap();
        let r = call("check", json!({ "path": root.to_string_lossy() }));
        let s = &r["structuredContent"];
        assert_eq!(s["summary"]["byCode"]["undefined-global"], 1, "{s}");
        assert_eq!(s["diagnostics"][0]["file"], "app/s.lua");
        // Unsaved text replaces the file.
        let file = res.join("s.lua");
        let r = call("check", json!({ "path": file.to_string_lossy(), "text": "local x = GetHashKey('a')\nprint(x)\n", "level": "hint", "fix": true }));
        let s = &r["structuredContent"];
        assert_eq!(s["fixedText"], "local x = joaat('a')\nprint(x)\n", "{s}");
        let r = call("resource_info", json!({ "path": file.to_string_lossy() }));
        let s = &r["structuredContent"];
        assert_eq!((s["resource"].as_str(), s["file"]["side"].as_str()), (Some("app"), Some("server")), "{s}");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn exports_members_events() {
        let root = std::env::temp_dir().join(format!("cfx-mcp-api-{}", std::process::id()));
        let core = root.join("core");
        let app = root.join("app");
        std::fs::create_dir_all(&core).unwrap();
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(core.join("fxmanifest.lua"), "server_script 's.lua'\nclient_script 'c.lua'\n").unwrap();
        std::fs::write(
            core.join("s.lua"),
            "Core = { Functions = {} }\n---Gets a player.\nfunction Core.Functions.GetPlayer(src) end\n---@deprecated use GetPlayer\nfunction Core.Functions.Old() end\nexports('GetCoreObject', function() return Core end)\nRegisterNetEvent('core:hello', function() end)\n",
        )
        .unwrap();
        std::fs::write(core.join("c.lua"), "exports('Notify', function(text) end)\nTriggerServerEvent('core:hello')\n").unwrap();
        std::fs::write(app.join("fxmanifest.lua"), "server_script 's.lua'\n").unwrap();
        std::fs::write(app.join("s.lua"), "local C = exports.core:GetCoreObject()\n").unwrap();
        let folder = root.to_string_lossy().to_string();

        let r = call("exports", json!({ "folder": folder, "resource": "core", "side": "server" }));
        let s = &r["structuredContent"];
        assert_eq!(s["exports"][0]["name"], "GetCoreObject", "{s}");
        assert_eq!(s["otherSide"][0]["signature"], "Notify(text)", "{s}");
        assert_eq!(s["otherSide"][0]["side"], "client");
        assert_eq!(s["complete"], true);

        let file = app.join("s.lua");
        let r = call("members", json!({ "folder": folder, "path": file.to_string_lossy(), "expression": "C.Functions" }));
        let s = &r["structuredContent"];
        assert_eq!(s["members"][0]["signature"], "GetPlayer(src)", "{s}");
        assert_eq!(s["members"][0]["doc"], "Gets a player.");
        assert_eq!(s["members"][0]["defined"], "core/s.lua:3");
        assert_eq!(s["members"][1]["deprecated"], "use GetPlayer");
        let r = call("members", json!({ "folder": folder, "path": file.to_string_lossy(), "expression": "exports.core:GetCoreObject()" }));
        assert_eq!(r["structuredContent"]["members"][0]["name"], "Functions", "{r}");

        let r = call("events", json!({ "folder": folder, "name": "core:hello" }));
        let s = &r["structuredContent"];
        assert_eq!((s["registered"][0].as_str(), s["triggered"][0].as_str()), (Some("core/s.lua:7"), Some("core/c.lua:2")), "{s}");
        let r = call("events", json!({ "folder": folder, "query": "HELLO" }));
        assert_eq!(r["structuredContent"]["total"], 1);

        let r = call("symbols", json!({ "folder": folder, "query": "getplayer" }));
        let s = &r["structuredContent"]["symbols"][0];
        assert_eq!((s["name"].as_str(), s["container"].as_str(), s["kind"].as_str()), (Some("GetPlayer"), Some("Core.Functions"), Some("function")), "{r}");
        assert_eq!(s["defined"], "core/s.lua:3");
        let r = call("symbols", json!({ "folder": folder, "query": "core:hel" }));
        assert_eq!(r["structuredContent"]["symbols"][0]["kind"], "event", "{r}");

        // The kept workspace follows the disk: a new export, then a deleted file.
        std::fs::write(core.join("c.lua"), "exports('Notify', function(text) end)\nexports('Toast', function() end)\n").unwrap();
        let r = call("exports", json!({ "folder": folder, "resource": "core", "side": "client" }));
        let names: Vec<&str> = r["structuredContent"]["exports"].as_array().unwrap().iter().filter_map(|e| e["name"].as_str()).collect();
        assert!(names.contains(&"Toast"), "{r}");
        std::fs::remove_file(core.join("c.lua")).unwrap();
        let r = call("exports", json!({ "folder": folder, "resource": "core", "side": "client" }));
        assert!(r["structuredContent"]["otherSide"].is_array() && !r.to_string().contains("Toast"), "{r}");

        let r = call("resource_info", json!({ "path": core.join("s.lua").to_string_lossy() }));
        assert_eq!(r["structuredContent"]["exports"][0]["name"], "GetCoreObject", "{r}");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
