//! Rules about how scripts reach other resources and frameworks, from the file
//! alone. Facts checked against FiveM's `scheduler.lua` (exports, NUI) and the
//! qb-core / qbx_core copies in the test servers (player getters).

use super::{Fix, Linter};
use crate::names::Target;
use cfx_lua_syntax::SyntaxKind::*;
use cfx_lua_syntax::SyntaxNode;

pub(super) fn node(l: &mut Linter, n: &SyntaxNode) {
    match n.kind() {
        CALL_EXPR => {
            export_dot_call(l, n);
            nui_message(l, n);
        }
        LOCAL_STAT => unchecked_player(l, n),
        _ => {}
    }
}

/// `exports.res.Name(a)`: an export is a function taking `self` first
/// (`return function(self, ...)` in scheduler.lua), so `a` is dropped.
fn export_dot_call(l: &mut Linter, call: &SyntaxNode) {
    let Some(callee) = call.first_child().filter(|c| c.kind() == INDEX_EXPR) else { return };
    let Some(object) = callee.first_child() else { return };
    let Some(resource) = crate::api::exports_resource(l.text, l.names, &object) else { return };
    let args = Linter::args(call);
    let Some(first) = args.first() else { return };
    let first_text = l.src(first.text_range()).trim();
    if first_text == "nil" || first_text == l.src(object.text_range()).trim() {
        return;
    }
    let dot = callee.children_with_tokens().filter_map(|e| e.into_token()).find(|t| t.kind() == DOT);
    let name = crate::lint::field_name(&callee).map(|t| t.text().to_string()).unwrap_or_else(|| "export".into());
    let message = format!(
        "`exports.{resource}.{name}(...)` passes its first argument as `self`, so the export never gets it: call it with `:`."
    );
    match dot {
        Some(dot) => {
            let fix = Fix { title: "Call the export with `:`".into(), edits: vec![(dot.text_range(), ":".into())] };
            l.push_fix("fivem/export-dot-call", callee.text_range(), message, fix);
        }
        None => l.push("fivem/export-dot-call", callee.text_range(), message),
    }
}

/// `SendNUIMessage(json.encode(t))`: `SendNUIMessage` encodes its argument
/// itself, so the page receives a JSON string instead of an object.
fn nui_message(l: &mut Linter, call: &SyntaxNode) {
    if l.callee(call).as_deref() != Some("SendNUIMessage") {
        return;
    }
    let args = Linter::args(call);
    if args.first().is_some_and(|a| l.callee(a).as_deref() == Some("json.encode")) {
        l.push(
            "fivem/nui-message-encoded",
            args[0].text_range(),
            "`SendNUIMessage` already JSON-encodes its argument: with `json.encode` the page's `event.data` is a string, not the table. Pass the table (and drop any `JSON.parse` on the page).",
        );
    }
}

/// A QBCore / Qbox player getter: `X.Functions.GetPlayer(...)`,
/// `exports.qbx_core:GetPlayer(...)` and the `ByCitizenId` variants. Both
/// frameworks return `nil` for a player that is not loaded or has left.
fn is_player_getter(l: &Linter, e: &SyntaxNode) -> bool {
    let getter = |n: &str| matches!(n, "GetPlayer" | "GetPlayerByCitizenId");
    match e.kind() {
        CALL_EXPR => {
            let Some(callee) = e.first_child().filter(|c| c.kind() == INDEX_EXPR) else { return false };
            let Some(name) = crate::lint::field_name(&callee) else { return false };
            let functions = callee.first_child().filter(|o| o.kind() == INDEX_EXPR).and_then(|o| crate::lint::field_name(&o));
            getter(name.text()) && functions.is_some_and(|f| f.text() == "Functions")
        }
        METHOD_CALL_EXPR => {
            let (Some(object), Some(m)) = (e.first_child(), crate::api::method_name(e)) else { return false };
            getter(m.text())
                && crate::api::exports_resource(l.text, l.names, &object).is_some_and(|r| matches!(r.as_str(), "qbx_core" | "qb-core"))
        }
        _ => false,
    }
}

/// `local Player = QBCore.Functions.GetPlayer(src)` whose first use indexes it.
fn unchecked_player(l: &mut Linter, stat: &SyntaxNode) {
    let mut locals = stat.children().filter(|c| c.kind() == LOCAL_NAME);
    let (Some(local), None) = (locals.next(), locals.next()) else { return };
    let Some(values) = stat.children().find(|c| c.kind() == EXPR_LIST) else { return };
    let mut vals = values.children();
    let (Some(value), None) = (vals.next(), vals.next()) else { return };
    if !is_player_getter(l, &value) {
        return;
    }
    let Some(tok) = local.first_token() else { return };
    let Some(d) = l.names.decls.iter().position(|d| d.range == tok.text_range()) else { return };
    let Some(first) = l.names.refs.iter().find(|r| r.target == Target::Decl(d) && r.range.start() > stat.text_range().start()) else {
        return;
    };
    let range = first.range;
    let root = l.root.clone();
    let Some(name_expr) = crate::selection::token_at(&root, range.start()).right_biased().and_then(|t| t.parent()) else { return };
    let indexed = name_expr.parent().is_some_and(|p| {
        matches!(p.kind(), INDEX_EXPR | METHOD_CALL_EXPR) && p.first_child().is_some_and(|c| c == name_expr)
    });
    if indexed {
        l.push(
            "qb/unchecked-player",
            range,
            format!(
                "`{}` is `nil` when the player is not loaded or has left; check it first (`if not {} then return end`).",
                tok.text(),
                tok.text()
            ),
        );
    }
}
