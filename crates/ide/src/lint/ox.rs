//! ox_lib suggestions, only where fxmanifest.lua loads `@ox_lib/init.lua` for the
//! file's side. Facts checked against ox_lib 3.39 (`init.lua`,
//! `resource/cache/client.lua`): `cache.ped`, `cache.vehicle`, `cache.seat` and
//! `cache.weapon` exist on the client and are refreshed every 100 ms, and
//! `vehicle` / `weapon` are `false` (not `0` or the unarmed hash) when there is
//! none; `cache.playerId`, `cache.serverId` are set once on the client;
//! `cache.resource` and `cache.game` exist on both sides. Rewrites are offered
//! only where the value is the same.

use super::{Fix, Linter};
use crate::manifest::FileSide;
use cfx_lua_syntax::SyntaxKind::*;
use cfx_lua_syntax::SyntaxNode;

/// `a.b.c` for a chain of names and `.field`s, local or global (frameworks are
/// usually held in a local: `local QBCore = exports['qb-core']:GetCoreObject()`).
fn path(l: &Linter, expr: &SyntaxNode) -> Option<String> {
    match expr.kind() {
        NAME_EXPR => Some(l.src(expr.text_range()).to_string()),
        INDEX_EXPR => Some(format!("{}.{}", path(l, &expr.first_child()?)?, super::field_name(expr)?.text())),
        _ => None,
    }
}

/// `PlayerPedId()`, `cache.ped` or `GetPlayerPed(-1)`: the local player's ped.
fn is_self_ped(l: &Linter, expr: &SyntaxNode) -> bool {
    match expr.kind() {
        CALL_EXPR => match l.callee(expr).as_deref() {
            Some("PlayerPedId") => true,
            Some("GetPlayerPed") => Linter::args(expr).first().is_some_and(|a| l.src(a.text_range()).trim() == "-1"),
            _ => false,
        },
        INDEX_EXPR => l.global_path(expr).as_deref() == Some("cache.ped"),
        _ => false,
    }
}

pub(super) fn node(l: &mut Linter, n: &SyntaxNode) {
    if !l.ctx.ox_lib {
        return;
    }
    match n.kind() {
        CALL_EXPR => call(l, n),
        WHILE_STAT | REPEAT_STAT => request_loop(l, n),
        _ => {}
    }
}

fn call(l: &mut Linter, call: &SyntaxNode) {
    let client = l.ctx.side == FileSide::Client;
    let r = call.text_range();
    let args = Linter::args(call);
    let replace = |with: &str| Fix { title: format!("Use {with}"), edits: vec![(r, with.to_string())] };
    if let Some(native) = l.callee(call) {
        match native.as_str() {
            "PlayerPedId" if client => {
                l.push_fix("ox/cache-ped", r, "ox_lib keeps the player's ped in `cache.ped`.", replace("cache.ped"));
            }
            "PlayerId" if client => {
                let in_server_id = call
                    .parent()
                    .and_then(|a| a.parent())
                    .is_some_and(|c| l.callee(&c).as_deref() == Some("GetPlayerServerId"));
                if !in_server_id {
                    l.push_fix("ox/cache-playerid", r, "ox_lib keeps the player id in `cache.playerId`.", replace("cache.playerId"));
                }
            }
            "GetPlayerServerId" if client => {
                let own = args.first().is_some_and(|a| {
                    l.callee(a).as_deref() == Some("PlayerId") || l.global_path(a).as_deref() == Some("cache.playerId")
                });
                if own && args.len() == 1 {
                    l.push_fix("ox/cache-serverid", r, "ox_lib keeps the player's server id in `cache.serverId`.", replace("cache.serverId"));
                }
            }
            "GetVehiclePedIsIn" if client => {
                if args.len() == 2 && is_self_ped(l, &args[0]) && l.src(args[1].text_range()).trim() == "false" {
                    l.push(
                        "ox/cache-vehicle",
                        r,
                        "ox_lib keeps the player's vehicle in `cache.vehicle` (refreshed every 100 ms); it is `false`, not `0`, on foot.",
                    );
                }
            }
            "GetSelectedPedWeapon" if client => {
                if args.len() == 1 && is_self_ped(l, &args[0]) {
                    l.push(
                        "ox/cache-weapon",
                        r,
                        "ox_lib keeps the current weapon in `cache.weapon` (refreshed every 100 ms); it is `false` when unarmed, where this native returns the unarmed hash.",
                    );
                }
            }
            "GetCurrentResourceName" if args.is_empty() => {
                l.push_fix("ox/cache-resource", r, "ox_lib keeps the resource name in `cache.resource`.", replace("cache.resource"));
            }
            "GetGamePool" if client => {
                let pool = args.first().map(|a| l.src(a.text_range()).trim_matches(['\'', '"', ' ']).to_string());
                let what = match pool.as_deref() {
                    Some("CVehicle") => Some("lib.getClosestVehicle` / `lib.getNearbyVehicles"),
                    Some("CPed") => Some("lib.getClosestPed` / `lib.getNearbyPeds"),
                    Some("CObject") => Some("lib.getClosestObject` / `lib.getNearbyObjects"),
                    _ => None,
                };
                if let Some(what) = what {
                    l.push("ox/closest", r, format!("ox_lib has `{what}` for searching the entity pool."));
                }
            }
            "BeginTextCommandThefeedPost" | "SetNotificationTextEntry" if client => {
                l.push("ox/notify", r, "ox_lib's `lib.notify` shows a notification in one call.");
            }
            "BeginTextCommandDisplayHelp" | "DisplayHelpTextThisFrame" if client => {
                l.push("ox/text-ui", r, "ox_lib's `lib.showTextUI` / `lib.hideTextUI` replace per-frame help text.");
            }
            "RegisterKeyMapping" if client => {
                l.push("ox/keybind", r, "ox_lib's `lib.addKeybind` registers the command and the key mapping together.");
            }
            "IsControlJustPressed" | "IsControlJustReleased" if client && l.in_loop() => {
                l.push("ox/keybind", r, "Polling a control every frame: ox_lib's `lib.addKeybind` reacts to the key without a loop.");
            }
            "StartShapeTestLosProbe" | "StartExpensiveSynchronousShapeTestLosProbe" if client => {
                l.push("ox/raycast", r, "ox_lib's `lib.raycast.fromCoords` / `lib.raycast.cam` run the shape test and wait for the result.");
            }
            "TriggerEvent" if args.first().is_some_and(|a| l.src(a.text_range()).trim_matches(['\'', '"']) == "chat:addSuggestion") => {
                l.push("ox/add-command", r, "ox_lib's `lib.addCommand` registers the command with its chat suggestion and permissions.");
            }
            _ => {}
        }
    }
    let Some(callee) = call.first_child() else { return };
    let Some(p) = path(l, &callee) else { return };
    let (code, msg) = match p.as_str() {
        "ESX.TriggerServerCallback" | "QBCore.Functions.TriggerCallback" => {
            ("ox/callback", "ox_lib's `lib.callback.await` (or `lib.callback`) replaces framework callbacks, and the server side gets `source` as its first argument.")
        }
        "ESX.RegisterServerCallback" | "QBCore.Functions.CreateCallback" => {
            ("ox/callback", "ox_lib's `lib.callback.register` replaces framework callbacks.")
        }
        "ESX.ShowNotification" | "QBCore.Functions.Notify" => ("ox/notify", "ox_lib's `lib.notify` shows a notification."),
        "ESX.ShowHelpNotification" => ("ox/text-ui", "ox_lib's `lib.showTextUI` / `lib.hideTextUI` replace per-frame help text."),
        "QBCore.Functions.Progressbar" | "ESX.Progressbar" => {
            ("ox/progress", "ox_lib's `lib.progressBar` / `lib.progressCircle` show a progress bar and return whether it completed.")
        }
        "ESX.Game.GetClosestVehicle" | "QBCore.Functions.GetClosestVehicle" => ("ox/closest", "ox_lib has `lib.getClosestVehicle`."),
        "ESX.Game.GetClosestPlayer" | "QBCore.Functions.GetClosestPlayer" => ("ox/closest", "ox_lib has `lib.getClosestPlayer`."),
        "ESX.Game.GetClosestPed" | "QBCore.Functions.GetClosestPed" => ("ox/closest", "ox_lib has `lib.getClosestPed`."),
        "ESX.Game.GetClosestObject" | "QBCore.Functions.GetClosestObject" => ("ox/closest", "ox_lib has `lib.getClosestObject`."),
        _ => return,
    };
    l.push(code, r, msg);
}

/// `while not HasModelLoaded(m) do Wait(0) end` / `repeat ... until HasModelLoaded(m)`.
fn request_loop(l: &mut Linter, stat: &SyntaxNode) {
    let cond = match stat.kind() {
        WHILE_STAT => stat.children().find(|c| c.kind() != BLOCK).and_then(|c| {
            // `not X(...)`
            (c.kind() == UNARY_EXPR && c.first_token().is_some_and(|t| t.kind() == NOT_KW)).then(|| c.first_child()).flatten()
        }),
        _ => stat.children().filter(|c| c.kind() != BLOCK).last(),
    };
    let Some(cond) = cond.filter(|c| c.kind() == CALL_EXPR) else { return };
    let Some(native) = l.callee(&cond) else { return };
    let helper = match native.as_str() {
        "HasModelLoaded" => "lib.requestModel",
        "HasAnimDictLoaded" => "lib.requestAnimDict",
        "HasAnimSetLoaded" => "lib.requestAnimSet",
        "HasNamedPtfxAssetLoaded" => "lib.requestNamedPtfxAsset",
        "HasStreamedTextureDictLoaded" => "lib.requestStreamedTextureDict",
        "HasScaleformMovieLoaded" => "lib.requestScaleformMovie",
        "HasWeaponAssetLoaded" => "lib.requestWeaponAsset",
        _ => return,
    };
    let Some(kw) = stat.first_token() else { return };
    let kw = stat
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| matches!(t.kind(), WHILE_KW | REPEAT_KW))
        .unwrap_or(kw);
    l.push(
        "ox/request-asset",
        kw.text_range(),
        format!("ox_lib's `{helper}` requests the asset and waits for it, with a timeout, in one call."),
    );
}
