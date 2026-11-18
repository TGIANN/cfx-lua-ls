//! FiveM rules (`docs/RULES.md`, "FiveM API").

use super::{Fix, Frame, Linter, is_yield};
use crate::library::{Origin, Side, library};
use crate::manifest::FileSide;
use crate::names::{Access, Target};
use crate::util::trimmed;
use crate::TextRange;
use cfx_lua_syntax::SyntaxKind::*;
use cfx_lua_syntax::{SyntaxNode, SyntaxToken};

/// Calls that give a player money, items, weapons, a job or permissions: a
/// value the client chose must not reach them unchecked (lower case).
const SENSITIVE: &[&str] = &[
    "addmoney",
    "addaccountmoney",
    "setmoney",
    "setaccountmoney",
    "addinventoryitem",
    "additem",
    "addweapon",
    "giveweapon",
    "setjob",
    "setgroup",
    "addpermission",
    "setpermission",
];

/// The name a call reaches: `x:addMoney()`, `x.addMoney()`, `exports.ox_inventory:AddItem()`.
fn called_name(call: &SyntaxNode) -> Option<String> {
    let tok = match call.kind() {
        METHOD_CALL_EXPR => crate::api::method_name(call)?,
        CALL_EXPR => super::field_name(&call.first_child().filter(|c| c.kind() == INDEX_EXPR)?)?,
        _ => return None,
    };
    Some(tok.text().to_string())
}

/// A server net event handler (`RegisterNetEvent(name, fn)`, or
/// `AddEventHandler(name, fn)` for a name the file registers as a net event)
/// passing one of its parameters, read nowhere before, straight into a money,
/// item, weapon, job or permission call.
fn unchecked_net_arg(l: &mut Linter, func: &SyntaxNode) {
    if l.ctx.side != FileSide::Server {
        return;
    }
    let Some(call) = func.parent().filter(|p| p.kind() == ARG_LIST).and_then(|a| a.parent()) else { return };
    let net = match l.callee(&call).as_deref() {
        Some("RegisterNetEvent" | "RegisterServerEvent") => true,
        Some("AddEventHandler") => {
            let name = Linter::args(&call).first().filter(|a| a.kind() == LITERAL_EXPR).map(|a| l.src(a.text_range()).trim().to_string());
            name.is_some_and(|n| {
                let n = crate::util::unquote(&n).unwrap_or_default().to_string();
                l.root.descendants().filter(|c| c.kind() == CALL_EXPR).any(|c| {
                    matches!(l.callee(&c).as_deref(), Some("RegisterNetEvent" | "RegisterServerEvent"))
                        && Linter::args(&c)
                            .first()
                            .and_then(|a| a.first_token())
                            .and_then(|t| crate::util::unquote(t.text()).map(str::to_string))
                            .is_some_and(|x| x == n)
                })
            })
        }
        _ => false,
    };
    if !net {
        return;
    }
    let Some(params) = func.children().find(|c| c.kind() == FUNC_BODY).and_then(|b| b.children().find(|c| c.kind() == PARAM_LIST)) else { return };
    let prange = params.text_range();
    let decls: Vec<usize> =
        l.names.decls.iter().enumerate().filter(|(_, d)| d.kind == crate::names::DeclKind::Param && prange.contains_range(d.range)).map(|(i, _)| i).collect();
    if decls.is_empty() {
        return;
    }
    let mut found = Vec::new();
    for c in func.descendants().filter(|c| matches!(c.kind(), CALL_EXPR | METHOD_CALL_EXPR)) {
        let Some(name) = called_name(&c) else { continue };
        if !SENSITIVE.contains(&name.to_ascii_lowercase().as_str()) {
            continue;
        }
        // `setJob(name, grade, onDuty)`: the duty flag is the player's own choice.
        let checked = if name.eq_ignore_ascii_case("setjob") { 2 } else { usize::MAX };
        for arg in Linter::args(&c).into_iter().take(checked).filter(|a| a.kind() == NAME_EXPR) {
            let Some(tok) = arg.first_token() else { continue };
            let Some(r) = l.names.refs.iter().find(|r| r.range == tok.text_range()) else { continue };
            let Target::Decl(d) = r.target else { continue };
            if !decls.contains(&d) {
                continue;
            }
            // Read before (a check, a conversion, a lookup): left alone.
            let earlier = l.names.refs.iter().any(|o| o.target == Target::Decl(d) && o.range.start() < c.text_range().start());
            if !earlier {
                found.push((arg.text_range(), l.names.decls[d].name.clone(), name.clone()));
            }
        }
    }
    for (range, param, call) in found {
        l.push(
            "fivem/unchecked-net-arg",
            range,
            format!("`{param}` comes from the client and goes into `{call}` unchecked: validate it on the server first (type, range, ownership)."),
        );
    }
}

/// A function passed to `RegisterNetEvent` / `AddEventHandler`: the global
/// `source` is the event's sender until the handler first yields.
pub(super) fn is_event_handler(l: &Linter, body: &SyntaxNode) -> bool {
    let Some(func) = body.parent().filter(|p| p.kind() == FUNCTION_EXPR) else { return false };
    let Some(call) = func.parent().filter(|p| p.kind() == ARG_LIST).and_then(|a| a.parent()) else { return false };
    l.callee(&call).is_some_and(|p| matches!(p.as_str(), "RegisterNetEvent" | "AddEventHandler" | "RegisterServerEvent"))
}

pub(super) fn token(l: &mut Linter, t: &SyntaxToken) {
    if t.kind() == HASH_STRING {
        let s = l.src(t.text_range());
        let inner = s.trim_matches('`');
        let fix = Fix { title: format!("Use joaat(\"{inner}\")"), edits: vec![(t.text_range(), format!("joaat(\"{inner}\")"))] };
        l.push_fix("fivem/backtick-hash", t.text_range(), format!("Write hashes with `joaat(\"{inner}\")` instead of a backtick string."), fix);
    }
}

pub(super) fn node(l: &mut Linter, n: &SyntaxNode) {
    match n.kind() {
        CALL_EXPR => call(l, n),
        NAME_EXPR => {
            if l.src(n.text_range()) == "source"
                && l.is_global_name(n)
                && let Some((handler, yielded)) = l.function()
                && *handler
                && *yielded
            {
                l.push(
                    "fivem/source-after-wait",
                    n.text_range(),
                    "`source` is no longer the event's sender after a `Wait` or other yield in a handler: copy it to a local on the handler's first line (`local src = source`).",
                );
            }
        }
        WHILE_STAT | REPEAT_STAT => loop_without_wait(l, n),
        FUNCTION_EXPR => unchecked_net_arg(l, n),
        BINARY_EXPR => {
            let op = n.children_with_tokens().filter_map(|e| e.into_token()).find(|t| matches!(t.kind(), EQ2 | NEQ));
            if op.is_some() {
                for operand in n.children() {
                    unsigned_hash(l, &operand);
                }
            }
        }
        FIELD_INDEXED => {
            if let Some(key) = n.children().next() {
                unsigned_hash(l, &key);
            }
        }
        _ => {}
    }
}

fn call(l: &mut Linter, call: &SyntaxNode) {
    let Some(path) = l.callee(call) else { return };
    let Some(callee) = call.first_child() else { return };
    let callee_r = callee.text_range();
    let args = Linter::args(call);
    match path.as_str() {
        "GetHashKey" => {
            let fix = Fix { title: "Use joaat".into(), edits: vec![(callee_r, "joaat".into())] };
            l.push_fix("fivem/gethashkey", callee_r, "`joaat(x)` is the Lua hash function; `GetHashKey` is a native call.", fix);
        }
        "Citizen.Wait" | "Citizen.CreateThread" | "Citizen.SetTimeout" => {
            let short = path.trim_start_matches("Citizen.").to_string();
            let fix = Fix { title: format!("Use {short}"), edits: vec![(callee_r, short.clone())] };
            l.push_fix("fivem/citizen-prefix", callee_r, format!("`{short}` is the same function without the `Citizen.` lookup."), fix);
        }
        "GetDistanceBetweenCoords" | "Vdist" | "Vdist2" => {
            l.push(
                "fivem/native-distance",
                callee_r,
                format!("`{path}` is a native call; vector math is faster: `#(a - b)` (or `#(a.xy - b.xy)` without the height)."),
            );
        }
        "GetPlayerPed" => {
            if args.len() == 1 && l.src(args[0].text_range()).trim() == "-1" {
                let ox = l.ctx.ox_lib && l.ctx.side == FileSide::Client;
                let with = if ox { "cache.ped" } else { "PlayerPedId()" };
                let r = call.text_range();
                let fix = Fix { title: format!("Use {with}"), edits: vec![(r, with.into())] };
                l.push_fix("fivem/getplayerped-self", r, format!("`GetPlayerPed(-1)` is the local player's ped: use `{with}`."), fix);
            }
        }
        "PlayerPedId" | "PlayerId" if !l.ctx.ox_lib => {
            let finite = l.frames.last().map(|f| match f {
                Frame::Loop { finite } => *finite,
                Frame::Function { .. } => false,
            });
            if finite == Some(true) {
                l.push(
                    "fivem/invariant-native-in-loop",
                    call.text_range(),
                    format!("`{path}()` returns the same value on every iteration: call it once before the loop."),
                );
            }
        }
        _ => {}
    }
    sandbox(l, &path, call, &args);
    wrong_side(l, &path, callee_r);
}

/// A string literal's contents.
fn literal_str(l: &Linter, expr: &SyntaxNode) -> Option<String> {
    let t = expr.first_token().filter(|t| t.kind() == STRING && expr.kind() == LITERAL_EXPR)?;
    let s = l.src(t.text_range());
    crate::util::unquote(s).map(str::to_string)
}

/// FiveM's Lua sandbox (docs.fivem.net/docs/developers/sandbox): blocked calls,
/// the `ls` / `dir`-only `io.popen`, no `..` in paths, no writes into another
/// resource without `add_filesystem_permission`, `io.readdir` returns a handle,
/// `os.setlocale` cannot change the locale.
fn sandbox(l: &mut Linter, path: &str, call: &SyntaxNode, args: &[SyntaxNode]) {
    let r = call.text_range();
    match path {
        "os.execute" | "io.tmpfile" => {
            l.push("fivem/sandbox-blocked", r, format!("`{path}` is blocked by the FiveM sandbox (error 13, Permission denied)."));
        }
        "io.popen" => {
            if let Some(cmd) = args.first().and_then(|a| literal_str(l, a)) {
                let first = cmd.split_whitespace().next().unwrap_or("");
                if !matches!(first, "ls" | "dir") {
                    l.push("fivem/sandbox-blocked", r, "The FiveM sandbox only runs emulated `ls` and `dir` through `io.popen`.");
                }
            }
        }
        "os.setlocale" if !args.is_empty() => {
            l.push("fivem/sandbox-setlocale", r, "`os.setlocale` cannot change the locale in FiveM; it only returns the current one.");
        }
        "pairs" | "ipairs" if args.first().and_then(|a| l.callee(a)).as_deref() == Some("io.readdir") => {
            l.push(
                "fivem/readdir-pairs",
                r,
                "`io.readdir` returns a directory handle, not a table: iterate it with `for name in dir:lines() do`.",
            );
        }
        _ => {}
    }
    // Paths of file functions: the first argument, except `SaveResourceFile(resource, file, ...)`.
    let file_arg = match path {
        "io.open" | "io.lines" | "io.readdir" | "os.remove" | "os.rename" => args.first(),
        "SaveResourceFile" | "LoadResourceFile" => args.get(1),
        _ => return,
    };
    if let Some(p) = file_arg.and_then(|a| literal_str(l, a)) {
        let p = p.replace('\\', "/");
        if p.split('/').any(|seg| seg == "..") {
            l.push("fivem/sandbox-path", r, "The FiveM sandbox rejects paths with `..`: use `@resource/path`.");
        }
    }
    let target = match path {
        "io.open" => {
            let write = args.get(1).and_then(|m| literal_str(l, m)).is_some_and(|m| m.contains(['w', 'a', '+']));
            let p = args.first().and_then(|a| literal_str(l, a));
            write.then(|| p.and_then(|p| p.strip_prefix('@').and_then(|r| r.split('/').next()).map(str::to_string))).flatten()
        }
        "SaveResourceFile" => args.first().and_then(|a| literal_str(l, a)),
        _ => None,
    };
    if let (Some(target), Some(own)) = (target, l.ctx.resource)
        && !target.eq_ignore_ascii_case(own)
    {
        l.push(
            "fivem/sandbox-write",
            r,
            format!("The FiveM sandbox blocks writing into another resource (`{target}`) unless server.cfg has `add_filesystem_permission {own} write {target}`."),
        );
    }
}

fn wrong_side(l: &mut Linter, path: &str, range: TextRange) {
    let file = l.ctx.side;
    if !matches!(file, FileSide::Client | FileSide::Server) || path.contains('.') || (l.ctx.defined_elsewhere)(path) {
        return;
    }
    // The file defines a global of that name itself (a wrapper or a stub).
    let written = l.names.refs.iter().any(|r| r.target == Target::Global && r.access == Access::Write && l.src(r.range) == path);
    if written {
        return;
    }
    let items = library().get(path);
    let natives: Vec<_> = items.iter().filter(|i| i.origin == Origin::Native).collect();
    if natives.is_empty() || items.iter().any(|i| i.origin != Origin::Native) {
        return;
    }
    let sides: Vec<Side> = natives.iter().map(|i| i.side).collect();
    let (native_side, file_side) = match file {
        FileSide::Client if sides.iter().all(|&s| s == Side::Server) => ("server", "client"),
        FileSide::Server if sides.iter().all(|&s| s == Side::Client) => ("client", "server"),
        _ => return,
    };
    l.push(
        "fivem/wrong-side-native",
        range,
        format!("`{path}` is a {native_side} native, and fxmanifest.lua loads this file on the {file_side}."),
    );
}

/// A `while true` / `repeat ... until false` loop whose body never reaches a
/// yield, `break`, `return` or `goto` and calls nothing that could yield
/// (only library functions): it freezes the game or server.
fn loop_without_wait(l: &mut Linter, stat: &SyntaxNode) {
    let forever = match stat.kind() {
        WHILE_STAT => stat.children().find(|c| c.kind() != BLOCK).is_some_and(|c| l.src(c.text_range()).trim() == "true"),
        _ => stat.children().filter(|c| c.kind() != BLOCK).last().is_some_and(|c| l.src(c.text_range()).trim() == "false"),
    };
    if !forever {
        return;
    }
    let Some(body) = stat.children().find(|c| c.kind() == BLOCK) else { return };
    let lib = library();
    let mut walk = body.preorder();
    while let Some(event) = walk.next() {
        let rowan::WalkEvent::Enter(n) = event else { continue };
        match n.kind() {
            FUNC_BODY => walk.skip_subtree(),
            BREAK_STAT | RETURN_STAT | GOTO_STAT | METHOD_CALL_EXPR => return,
            CALL_EXPR => match l.callee(&n) {
                Some(p) if !is_yield(&p) && !lib.get(&p).is_empty() => {}
                _ => return,
            },
            _ => {}
        }
    }
    let Some(kw) = stat.first_token() else { return };
    let kw = stat
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| matches!(t.kind(), WHILE_KW | REPEAT_KW))
        .unwrap_or(kw);
    l.push(
        "fivem/loop-without-wait",
        kw.text_range(),
        "This loop never yields: add `Wait(ms)` inside it, or it blocks the thread forever.",
    );
}

const SIGNED_LIMIT: u64 = 1 << 31;

fn unsigned_hash(l: &mut Linter, expr: &SyntaxNode) {
    if expr.kind() != LITERAL_EXPR {
        return;
    }
    let Some(t) = expr.first_token().filter(|t| t.kind() == NUMBER) else { return };
    let s = l.src(t.text_range());
    let v = match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(h) => u64::from_str_radix(h, 16).ok(),
        None => s.parse::<u64>().ok(),
    };
    let Some(v) = v.filter(|&v| (SIGNED_LIMIT..1 << 32).contains(&v)) else { return };
    let signed = v as i64 - (1 << 32);
    let r = trimmed(expr).unwrap_or(t.text_range());
    let fix = Fix { title: format!("Use {signed}"), edits: vec![(r, signed.to_string())] };
    l.push_fix(
        "fivem/unsigned-hash",
        r,
        format!("FiveM hashes are signed 32-bit integers, so `{s}` never matches one: use `{signed}`."),
        fix,
    );
}
