//! Rule tests: each case lists `(code, flagged text)` pairs.

use super::*;
use crate::{names, parse};

fn run(text: &str, ctx: &Context) -> Vec<(&'static str, String)> {
    let p = parse(text);
    let n = names(&p, text);
    lint(&p, text, &n, ctx).into_iter().map(|d| (d.code, text[d.range].to_string())).collect()
}

fn codes(text: &str) -> Vec<(&'static str, String)> {
    run(text, &Context::default())
}

fn has(found: &[(&'static str, String)], code: &str, at: &str) -> bool {
    found.iter().any(|(c, t)| *c == code && t == at)
}

fn only(found: &[(&'static str, String)], code: &str) -> Vec<String> {
    found.iter().filter(|(c, _)| *c == code).map(|(_, t)| t.clone()).collect()
}

fn client_ox() -> Context<'static> {
    Context { side: FileSide::Client, ox_lib: true, ..Context::default() }
}

#[test]
fn unused_locals_as_luals() {
    let f = codes("local a = 1\nlocal t = {}\nt.x = 1\nlocal u = get()\nu.x = 1\nlocal _ = 2\nlocal function f(p) end\nprint(xx)\n");
    assert_eq!(only(&f, "unused-local"), ["a", "t", "t", "f", "p"], "{f:?}");
    // `xx` and `get` are undefined; `print` is in the library.
    assert_eq!(only(&f, "undefined-global"), ["get", "xx"]);
    let f = codes("local x <close> = nil\nlocal function g(...) end\ng()\n::top::\n");
    assert!(only(&f, "unused-local").is_empty());
    assert_eq!(only(&f, "unused-vararg"), ["..."]);
    assert_eq!(only(&f, "unused-label"), ["top"]);
}

#[test]
fn globals() {
    let f = codes("count = 1\nCount = 2\nprint(count, Count, Undefined)\nfunction helper() end\n");
    assert_eq!(only(&f, "lowercase-global"), ["count", "helper"]);
    assert_eq!(only(&f, "undefined-global"), ["Undefined"]);
    // ox_lib's globals: defined when the manifest loads it, explained when not.
    let f = codes("print(lib.x, cache.ped)\n");
    assert_eq!(only(&f, "undefined-global"), ["lib", "cache"]);
    assert!(run("print(lib.x, cache.ped)\n", &client_ox()).iter().all(|(c, _)| *c != "undefined-global"));
    let defined = |n: &str| n == "Shared";
    let ctx = Context { defined_elsewhere: &defined, ..Context::default() };
    assert!(run("print(Shared)\n", &ctx).is_empty());
    let ctx = Context { globals_unknown: true, ..Context::default() };
    assert!(run("print(Anything)\n", &ctx).is_empty());
}

#[test]
fn luals_shape_rules() {
    let f = codes("local x = 1\ndo local x = 2 print(x) end print(x)\nif x then end\nfor i = 10, 1 do print(i) end\nfor i = #t, 1 do print(i) end\nlocal a, b = 1\nlocal c = 1, 2\nprint(a, b, c)\n");
    assert_eq!(only(&f, "redefined-local"), ["x"]);
    assert_eq!(only(&f, "empty-block"), ["if x then end"]);
    assert_eq!(only(&f, "count-down-loop"), ["10, 1", "#t, 1"]);
    let g = codes("for dx = -1, 1 do print(dx) end\nfor i = 1, -1 do print(i) end\nwhile true do break; end\n");
    assert_eq!(only(&g, "count-down-loop"), ["1, -1"]);
    assert!(only(&g, "code-after-break").is_empty());
    assert_eq!(only(&f, "unbalanced-assignments"), ["b"]);
    assert_eq!(only(&f, "redundant-value"), ["2"]);
    let f = codes("t = { a = 1, a = 2, [1] = 'x', 'y' }\nwhile true do break print(1) end\nf\n(g)()\nx = 1 \n");
    assert_eq!(only(&f, "duplicate-index"), ["a", "a", "1", "'y'"]);
    assert!(has(&f, "code-after-break", "print(1)"));
    assert!(has(&f, "newline-call", "f\n(g)"));
    assert!(has(&f, "trailing-space", " "));
}

#[test]
fn suppression_comments() {
    let text = "---@diagnostic disable-next-line: undefined-global\nprint(A)\nprint(B) ---@diagnostic disable-line\n---@diagnostic disable: undefined-global\nprint(C)\n---@diagnostic enable: undefined-global\nprint(D)\n";
    assert_eq!(only(&codes(text), "undefined-global"), ["D"]);
    let mut config = LintConfig::default();
    config.disable.insert("undefined-global".into());
    let ctx = Context { config: &config, ..Context::default() };
    assert!(run("print(X)\n", &ctx).is_empty());
    let mut config = LintConfig::default();
    config.severity.insert("undefined-global".into(), Some(Severity::Error));
    let ctx = Context { config: &config, ..Context::default() };
    let p = parse("print(X)\n");
    let n = names(&p, "print(X)\n");
    assert_eq!(lint(&p, "print(X)\n", &n, &ctx)[0].severity, Severity::Error);
}

#[test]
fn fivem_rules() {
    let f = codes("local h = `adder`\nlocal g = GetHashKey('x')\nCitizen.Wait(0)\nlocal d = Vdist(1,2,3,4,5,6)\nlocal p = GetPlayerPed(-1)\nprint(h, g, d, p)\n");
    assert!(has(&f, "fivem/backtick-hash", "`adder`"));
    assert!(has(&f, "fivem/gethashkey", "GetHashKey"));
    assert!(has(&f, "fivem/citizen-prefix", "Citizen.Wait"));
    assert!(has(&f, "fivem/native-distance", "Vdist"));
    assert!(has(&f, "fivem/getplayerped-self", "GetPlayerPed(-1)"));
    // A local named like a native is not the native.
    assert!(only(&codes("local function GetHashKey(x) return x end\nprint(GetHashKey('a'))\n"), "fivem/gethashkey").is_empty());
}

#[test]
fn loops_and_handlers() {
    let f = codes("CreateThread(function()\n  while true do\n    local p = PlayerPedId()\n    SetEntityHealth(p, 200)\n  end\nend)\n");
    assert_eq!(only(&f, "fivem/loop-without-wait"), ["while"]);
    // Waits, breaks or calls into unknown code (which may wait) are fine.
    for body in ["Wait(0)", "if x then break end", "Process()", "obj:step()"] {
        let text = format!("while true do\n  {body}\nend\n");
        assert!(only(&codes(&text), "fivem/loop-without-wait").is_empty(), "{body}");
    }
    let f = codes("for i = 1, 10 do print(PlayerPedId()) end\nwhile true do local p = PlayerPedId() Wait(0) end\nfor _, v in ipairs(GetNearby(PlayerPedId())) do print(v) end\n");
    assert_eq!(only(&f, "fivem/invariant-native-in-loop"), ["PlayerPedId()"]);
    // A `for` header runs once.
    let f = codes("for _, k in ipairs({ 'a', 'b' }) do print(k) end\nfor i = 1, 2 do print(table.concat({ 'a', 'b' })) end\n");
    assert_eq!(only(&f, "perf/constant-table-in-loop"), ["{ 'a', 'b' }"]);
    let f = codes(
        "RegisterNetEvent('a', function()\n  local src = source\n  Wait(100)\n  print(source)\n  local r = MySQL.query.await('x', { source })\nend)\n",
    );
    assert_eq!(only(&f, "fivem/source-after-wait"), ["source", "source"]);
    let f = codes("RegisterNetEvent('a', function()\n  local r = MySQL.query.await('x', { source })\nend)\n");
    assert!(only(&f, "fivem/source-after-wait").is_empty());
}

#[test]
fn hashes_and_sides() {
    let f = codes("if model == 2971866336 then end\nlocal t = { [3000000000] = 1 }\nprint(t)\n");
    assert_eq!(only(&f, "fivem/unsigned-hash"), ["2971866336", "3000000000"]);
    let server = Context { side: FileSide::Server, ..Context::default() };
    let f = run("local p = PlayerPedId()\nprint(p, GetPlayerName(1))\n", &server);
    assert_eq!(only(&f, "fivem/wrong-side-native"), ["PlayerPedId"]);
    let client = Context { side: FileSide::Client, ..Context::default() };
    let f = run("print(GetPlayerName(1))\nDropPlayer(1, 'x')\n", &client);
    assert_eq!(only(&f, "fivem/wrong-side-native"), ["DropPlayer"]);
    // A global the file defines itself is not the native.
    let f = run("function DropPlayer(id) print(id) end\nDropPlayer(1)\n", &client);
    assert!(only(&f, "fivem/wrong-side-native").is_empty());
}

#[test]
fn sandbox() {
    let ctx = Context { resource: Some("myres"), ..Context::default() };
    let f = run(
        "os.execute('rm -rf /')\nlocal h = io.popen('curl x')\nlocal ok = io.popen('dir')\nlocal f = io.open('@myres/../x.txt')\nlocal w = io.open('@other/data.json', 'w')\nlocal own = io.open('@myres/data.json', 'w')\nSaveResourceFile('other', 'a.json', '{}', -1)\nfor k in pairs(io.readdir('@myres')) do print(k) end\nos.setlocale('C')\nprint(h, ok, f, w, own)\n",
        &ctx,
    );
    assert_eq!(only(&f, "fivem/sandbox-blocked"), ["os.execute('rm -rf /')", "io.popen('curl x')"]);
    assert_eq!(only(&f, "fivem/sandbox-path"), ["io.open('@myres/../x.txt')"]);
    assert_eq!(only(&f, "fivem/sandbox-write"), ["io.open('@other/data.json', 'w')", "SaveResourceFile('other', 'a.json', '{}', -1)"]);
    assert_eq!(only(&f, "fivem/readdir-pairs"), ["pairs(io.readdir('@myres'))"]);
    assert_eq!(only(&f, "fivem/sandbox-setlocale"), ["os.setlocale('C')"]);
}

#[test]
fn exports_nui_and_framework_rules() {
    let client = Context { side: FileSide::Client, ..Context::default() };
    let text = "exports.res.Get(1)\nexports['res'].Get(exports['res'])\nexports.res.Get()\nexports.res:Get(1)\nSendNUIMessage(json.encode({ a = 1 }))\nSendNUIMessage({ a = 1 })\n";
    let f = run(text, &client);
    assert_eq!(only(&f, "fivem/export-dot-call"), ["exports.res.Get"]);
    assert_eq!(only(&f, "fivem/nui-message-encoded"), ["json.encode({ a = 1 })"]);
    // A module's own `exports` table is not FiveM's; one set from the global is.
    let f = run("local exports = {}\nexports.util.fn(1)\n", &client);
    assert!(only(&f, "fivem/export-dot-call").is_empty(), "{f:?}");
    let f = run("local exports = assert(exports)\nexports.res.Get(1)\n", &client);
    assert_eq!(only(&f, "fivem/export-dot-call"), ["exports.res.Get"]);
    let p = parse(text);
    let n = names(&p, text);
    let (fixed, _) = apply_fixes(text, &lint(&p, text, &n, &client));
    assert!(fixed.starts_with("exports.res:Get(1)\n"), "{fixed}");

    let server = Context { side: FileSide::Server, ..Context::default() };
    let text = "local QBCore = {}\nlocal a = QBCore.Functions.GetPlayer(1)\nprint(a.PlayerData)\nlocal b = exports.qbx_core:GetPlayerByCitizenId('x')\nb.Functions.Save()\nlocal c = QBCore.Functions.GetPlayer(2)\nif not c then return end\nprint(c.PlayerData)\nlocal d = exports.qbx_core:GetPlayer(3)\nif d then print(d.PlayerData) end\n";
    let f = run(text, &server);
    assert_eq!(only(&f, "qb/unchecked-player"), ["a", "b"]);
}

#[test]
fn ox_lib_rules() {
    let text = "local ped = PlayerPedId()\nlocal id = GetPlayerServerId(PlayerId())\nlocal me = PlayerId()\nlocal veh = GetVehiclePedIsIn(PlayerPedId(), false)\nlocal res = GetCurrentResourceName()\nwhile not HasModelLoaded(m) do Wait(0) end\nQBCore.Functions.TriggerCallback('x', function() end)\nprint(ped, id, me, veh, res)\n";
    let f = run(text, &client_ox());
    assert_eq!(only(&f, "ox/cache-ped"), ["PlayerPedId()", "PlayerPedId()"]);
    assert_eq!(only(&f, "ox/cache-serverid"), ["GetPlayerServerId(PlayerId())"]);
    assert_eq!(only(&f, "ox/cache-playerid"), ["PlayerId()"]);
    assert_eq!(only(&f, "ox/cache-vehicle"), ["GetVehiclePedIsIn(PlayerPedId(), false)"]);
    assert_eq!(only(&f, "ox/cache-resource"), ["GetCurrentResourceName()"]);
    assert_eq!(only(&f, "ox/request-asset"), ["while"]);
    assert_eq!(only(&f, "ox/callback"), ["QBCore.Functions.TriggerCallback('x', function() end)"]);
    // Without ox_lib in the manifest: none of them.
    assert!(codes(text).iter().all(|(c, _)| !c.starts_with("ox/")));
    // The fix replaces the call.
    let text = "local ped = PlayerPedId()\nprint(ped)\n";
    let p = parse(text);
    let n = names(&p, text);
    let d = lint(&p, text, &n, &client_ox());
    let fix = d.iter().find(|d| d.code == "ox/cache-ped").and_then(|d| d.fix.clone()).unwrap();
    assert_eq!(fix.edits[0].1, "cache.ped");
}

#[test]
fn ox_lib_replacements() {
    let text = "\
local w = GetSelectedPedWeapon(PlayerPedId())
local other = GetSelectedPedWeapon(target)
BeginTextCommandThefeedPost('STRING')
ESX.ShowNotification('hi')
DisplayHelpTextThisFrame('x', false)
QBCore.Functions.Progressbar('a', 'b', 1000)
for _, v in ipairs(GetGamePool('CVehicle')) do print(v) end
local players = GetGamePool('CPlayer')
ESX.Game.GetClosestPlayer()
RegisterKeyMapping('cmd', 'desc', 'keyboard', 'e')
if IsControlJustPressed(0, 38) then print(1) end
CreateThread(function() while true do if IsControlJustPressed(0, 38) then print(2) end Wait(0) end end)
TriggerEvent('chat:addSuggestion', '/cmd', 'help')
TriggerEvent('other:event')
local h = StartShapeTestLosProbe(0, 0, 0, 1, 1, 1, -1, 0, 4)
print(w, other, players, h)
";
    let f = run(text, &client_ox());
    assert_eq!(only(&f, "ox/cache-weapon"), ["GetSelectedPedWeapon(PlayerPedId())"]);
    assert_eq!(only(&f, "ox/notify"), ["BeginTextCommandThefeedPost('STRING')", "ESX.ShowNotification('hi')"]);
    assert_eq!(only(&f, "ox/text-ui"), ["DisplayHelpTextThisFrame('x', false)"]);
    assert_eq!(only(&f, "ox/progress"), ["QBCore.Functions.Progressbar('a', 'b', 1000)"]);
    assert_eq!(only(&f, "ox/closest"), ["GetGamePool('CVehicle')", "ESX.Game.GetClosestPlayer()"]);
    // A key mapping, and a control polled in a loop; not a single test outside one.
    assert_eq!(only(&f, "ox/keybind"), ["RegisterKeyMapping('cmd', 'desc', 'keyboard', 'e')", "IsControlJustPressed(0, 38)"]);
    assert_eq!(only(&f, "ox/add-command"), ["TriggerEvent('chat:addSuggestion', '/cmd', 'help')"]);
    assert_eq!(only(&f, "ox/raycast"), ["StartShapeTestLosProbe(0, 0, 0, 1, 1, 1, -1, 0, 4)"]);
    // Client natives in a server file: none of the client suggestions.
    let server = Context { side: FileSide::Server, ox_lib: true, ..Context::default() };
    let f = run(text, &server);
    for code in ["ox/cache-weapon", "ox/text-ui", "ox/keybind", "ox/raycast"] {
        assert!(only(&f, code).is_empty(), "{code}: {f:?}");
    }
    // Without ox_lib: nothing.
    assert!(codes(text).iter().all(|(c, _)| !c.starts_with("ox/")));
}

#[test]
fn perf_and_style() {
    let f = codes("local s = ''\nfor i = 1, 3 do s = s .. i end\ntable.insert(list, 1)\nlocal t = { [1] = 'a', [2] = 'b' }\nlocal u = data[\"name\"]\nlocal fn = load(code)\nprint(s, t, u, fn)\n");
    assert!(has(&f, "perf/concat-in-loop", "s = s .. i"));
    assert!(has(&f, "perf/table-insert-append", "table.insert(list, 1)"));
    assert!(has(&f, "perf/sequential-index-keys", "{ [1] = 'a', [2] = 'b' }"));
    assert!(has(&f, "style/string-key-access", "[\"name\"]"));
    assert!(only(&codes("local e = exports[\"qb_core\"]\nprint(e)\n"), "style/string-key-access").is_empty());
    assert!(has(&f, "perf/runtime-load", "load(code)"));
    let f = codes("local function f(a)\n  if a > 1 then return true else return false end\nend\nprint(f)\n");
    assert_eq!(only(&f, "style/redundant-boolean-return").len(), 1);
    let f = codes("local my_value = 1\nlocal MAX <const> = 3\nlocal limit <const> = 5\nlocal GetEntityCoords <const> = GetEntityCoords\nlocal HTTP_OK = 200\nprint(my_value, MAX, limit, GetEntityCoords, HTTP_OK)\n");
    assert_eq!(only(&f, "naming/local"), ["my_value"]);
    assert_eq!(only(&f, "naming/const"), ["limit"]);
    // The rename is camelCase: a capitalized first part goes lower-case, an all-caps one stays.
    for (from, to) in [("my_value", "myValue"), ("Bad_Name", "badName"), ("ESX_data", "ESXData"), ("_private_x", "_privateX")] {
        let text = format!("local {from} = 1\nprint({from})\n");
        let p = parse(&text);
        let n = names(&p, &text);
        let d = lint(&p, &text, &n, &Context::default());
        let fix = d.iter().find(|d| d.code == "naming/local").and_then(|d| d.fix.clone()).unwrap();
        assert_eq!(fix.edits[0].1, to, "{from}");
    }
}

#[test]
fn flow_rules_beyond_luals() {
    // A `return` inside a loop does not make the rest unreachable: the loop may not run.
    let f = codes("local function f(t)\n    for _, v in pairs(t) do\n        return v\n    end\n    return nil\nend\nf({})\n");
    assert!(only(&f, "unreachable-code").is_empty(), "{f:?}");
    // `error()` in every branch does.
    let f = codes("local function f(x)\n    if x then error('a') else error('b') end\n    print(1)\nend\nf(1)\n");
    assert_eq!(only(&f, "unreachable-code"), ["print(1)"]);
    // A `goto` out of an inner loop leaves `while true` too.
    let f = codes("while true do\n    for i = 1, 2 do goto out end\nend\n::out::\nprint(1)\n");
    assert!(only(&f, "unreachable-code").is_empty(), "{f:?}");
    // A fallback message is not ambiguous; a fallback number is.
    let f = codes("local t, k = {}, 1\nprint(t[k] or 'Missing: ' .. k, t[k] or 0 + 1)\n");
    assert_eq!(only(&f, "ambiguity-1"), ["t[k] or 0 + 1"]);
    // Called from an anonymous callback: used.
    let f = codes("local function g() end\nCreateThread(function() g() end)\n");
    assert!(only(&f, "unused-function").is_empty(), "{f:?}");
}

#[test]
fn doc_rules_beyond_luals() {
    // Codes of other linters are left alone; a typo of a known one is not.
    let f = codes("---@diagnostic disable-next-line: unused-argument, qbox/prefer-cache, undefined-globl -- why\nprint(1)\n");
    assert_eq!(only(&f, "unknown-diag-code"), ["undefined-globl"], "{f:?}");
    // Docs on a table field's function, and across a plain comment line.
    let f = codes("Config = {\n    ---@param a integer\n    f = function(a) end,\n    ---@param b integer\n    -- note\n    g = function(b) end,\n}\n");
    assert!(only(&f, "undefined-doc-param").is_empty(), "{f:?}");
    // Docs that slid above the wrong function.
    let f = codes("---@param id string\n---@param note string\nlocal function clean(note) return note end\nclean('x')\n");
    assert_eq!(only(&f, "undefined-doc-param"), ["id"]);
    // `---` lines inside a long string are text (they once built a reversed range).
    let f = codes("local s = [[\n---@param x integer\n---@field y integer\n]]\nprint(s)\n");
    assert!(f.iter().all(|(c, _)| !c.contains("doc")), "{f:?}");
    // `self` of a method, `...`.
    let f = codes("T = {}\n---@param self table\n---@param x integer\nfunction T:m(x) end\n---@param any string\nlocal function v(...) end\nv()\n");
    assert!(only(&f, "undefined-doc-param").is_empty(), "{f:?}");
}

#[test]
fn return_rules_beyond_luals() {
    // An empty body is a declaration (ox_lib's `cache` stub), outside meta files too.
    let f = codes("---@return integer\nfunction cache() end\n");
    assert!(only(&f, "missing-return").is_empty(), "{f:?}");
    // A loop body that returns does not cover the end.
    let f = codes("---@return integer\nlocal function f(t)\n    for _ = 1, #t do\n        if t then return 1 end\n    end\nend\nf({})\n");
    assert_eq!(only(&f, "missing-return").len(), 1, "{f:?}");
    // A call last may give the rest.
    let f = codes("local function g() return 1, 2 end\n---@return integer, integer\nlocal function f() return g() end\nf()\n");
    assert!(only(&f, "missing-return-value").is_empty(), "{f:?}");
    // `---@return ...` takes any number.
    let f = codes("---@return ...\nlocal function f() return 1, 2, 3 end\nf()\n");
    assert!(only(&f, "redundant-return-value").is_empty(), "{f:?}");
}

#[test]
fn type_rules_beyond_luals() {
    // nil is not checked; a hash string is an integer; a truthy test drops `false`.
    let text = "---@param x number
local function f(x) end
f(nil)
f(`WEAPON_PISTOL`)
local v = GetVehicle() or false
if v then f(v) end
f('s')
";
    let f = codes(text);
    let hits = only(&f, "param-type-mismatch");
    assert_eq!(hits.len(), 1, "{f:?}");
    // A local typed by its first value is reused freely; a `---@type` one is not.
    let f = codes("local found = false
found = 1
---@type integer
local n = 1
n = 'a'
print(found, n)
");
    assert_eq!(only(&f, "cast-local-type"), ["n"], "{f:?}");
    // need-check-nil: checks before the use, `x?.f`, upvalues.
    let src = "---@param a? table
---@param b? table
---@param c? table
local function f(a, b, c)
    if type(a) ~= 'table' then return end
    print(a.x, b?.x, c.x)
    while type(b) == 'table' do b = b.next end
end
---@type table?
local u
local function g() return u.x end
f(); g()
";
    let f = codes(src);
    assert_eq!(only(&f, "need-check-nil"), ["c"], "{f:?}");
    // An `elseif` after `not x`, and `type(x) == 'a' or type(x) == 'b'`.
    let src = "---@param a? table\n---@param b? vector3\nlocal function f(a, b)\n    if not a then return 1 elseif a.on then return 2 end\n    return (type(b) == 'vector3' or type(b) == 'vector4') and b.xyz\nend\nf()\n";
    let f = codes(src);
    assert_eq!(only(&f, "need-check-nil"), Vec::<&str>::new(), "{f:?}");
}

#[test]
fn off_by_default_rules_turn_on() {
    let text = "---@async\nlocal function wait() end\nlocal function f()\n    wait()\nend\nlocal u\nfor i = 1, 2 do print(i) end\nf()\nprint(u)\n";
    let f = codes(text);
    assert!(only(&f, "await-in-sync").is_empty() && only(&f, "no-unknown").is_empty(), "{f:?}");
    let mut config = LintConfig::default();
    config.severity.insert("await-in-sync".into(), Some(Severity::Warning));
    config.severity.insert("no-unknown".into(), Some(Severity::Information));
    let ctx = Context { config: &config, ..Context::default() };
    let f = run(text, &ctx);
    assert_eq!(only(&f, "await-in-sync"), ["wait"], "{f:?}");
    assert_eq!(only(&f, "no-unknown"), ["u"], "{f:?}");
}

#[test]
fn unchecked_net_args() {
    let server = Context { side: FileSide::Server, ..Context::default() };
    // `item` and `amount` reach AddItem unread; `count` is checked first; a
    // local event (`playerDropped`) and the client side are trusted.
    let text = "RegisterNetEvent('a:give')\nAddEventHandler('a:give', function(item, amount)\n  local p = QBCore.Functions.GetPlayer(source)\n  p.Functions.AddItem(item, amount)\nend)\nRegisterNetEvent('a:pay', function(count)\n  if type(count) ~= 'number' or count < 1 then return end\n  xPlayer.addMoney(count)\nend)\nAddEventHandler('playerDropped', function(reason)\n  xPlayer.addMoney(reason)\nend)\nRegisterNetEvent('a:duty', function(onDuty)\n  xPlayer.setJob(xPlayer.job.name, xPlayer.job.grade, onDuty)\nend)\n";
    let f = run(text, &server);
    assert_eq!(only(&f, "fivem/unchecked-net-arg"), ["item", "amount"], "{f:?}");
    assert!(only(&codes(text), "fivem/unchecked-net-arg").is_empty());
}

#[test]
fn closures_and_globals_in_loops() {
    // The comparator uses nothing of the loop; the second captures `k`, the
    // third goes to an event, the fourth loop waits.
    let text = "local lists = {}\nfor _, l in pairs(lists) do\n  table.sort(l, function(a, b) return a < b end)\nend\nfor k in pairs(lists) do\n  table.sort(lists[k], function(a, b) return a[k] < b[k] end)\n  AddEventHandler('x', function() print(1) end)\nend\nwhile true do\n  pcall(function() print(1) end)\n  Wait(0)\nend\n";
    let f = codes(text);
    assert_eq!(only(&f, "perf/closure-in-loop"), ["function"], "{f:?}");
    assert!(only(&f, "perf/global-in-loop").is_empty(), "{f:?}");
    let mut config = LintConfig::default();
    config.severity.insert("perf/global-in-loop".into(), Some(Severity::Hint));
    let ctx = Context { config: &config, ..Context::default() };
    // Once per name and loop; not in a loop that waits.
    let f = run("for i = 1, 9 do\n  print(math.floor(i), math.floor(i / 2), math.max(i, 1))\nend\nfor i = 1, 9 do\n  print(math.floor(i))\n  Wait(0)\nend\n", &ctx);
    assert_eq!(only(&f, "perf/global-in-loop"), ["math.floor", "math.max"], "{f:?}");
}

#[test]
fn name_style_check() {
    let text = "local Mod = {}\nlocal a, B = 1, 2\nlocal C <const> = 3\nlocal Json = require 'json'\nGlobalX = 1\nMod.FieldY = 2\nfunction Mod.DoIt(Param, ...)\n    local Inner = Param\n    for K, v in pairs(Inner) do end\n    for I = 1, 2 do end\n    return function(Arg) end\nend\nlocal function __index() end\nlocal function LocalF() end\nreturn Mod\n";
    assert!(only(&codes(text), "name-style-check").is_empty(), "off by default");
    let mut config = LintConfig::default();
    config.severity.insert("name-style-check".into(), Some(Severity::Warning));
    let ctx = Context { config: &config, ..Context::default() };
    let found = run(text, &ctx);
    // `Mod` is the module (snake_case), only the first name of a top-level
    // `local a, B` is checked, `Json` passes as a required module and the `for`
    // index is never checked, as in EmmyLuaCodeStyle.
    assert_eq!(only(&found, "name-style-check"), ["Mod", "GlobalX", "FieldY", "DoIt", "Param", "Inner", "K", "Arg", "LocalF"]);
    let p = parse(text);
    let n = names(&p, text);
    let first = lint(&p, text, &n, &ctx).into_iter().find(|d| d.code == "name-style-check").unwrap();
    assert_eq!(first.message, "Name 'Mod' does not match rule 'ModuleName', which require 'snake-case'");
    config.name_style = name_style::NameStyleConfig::parse(&serde_json::json!({ "module_name_style": "pascal_case", "global_variable_name_style": ["pascal_case"] }));
    let ctx = Context { config: &config, ..Context::default() };
    let found = only(&run(text, &ctx), "name-style-check");
    assert!(!found.contains(&"Mod".to_string()) && !found.contains(&"GlobalX".to_string()));
}

#[test]
fn spell_check() {
    let text = "local plyerName = 'helo wrld here'\nlocal ok_name = \"x\tvalue\"\nprint(plyerName, ok_name)\n";
    assert!(only(&codes(text), "spell-check").is_empty(), "off by default");
    let mut config = LintConfig::default();
    config.severity.insert("spell-check".into(), Some(Severity::Information));
    let ctx = Context { config: &config, ..Context::default() };
    assert_eq!(only(&run(text, &ctx), "spell-check"), ["plyer", "helo", "wrld", "plyer"]);
    config.spell_dict = vec!["plyer".into()];
    let ctx = Context { config: &config, ..Context::default() };
    assert_eq!(only(&run(text, &ctx), "spell-check"), ["helo", "wrld"]);
    assert_eq!(spell_suggest("Plyer").first().map(String::as_str), Some("Player"));
}

#[test]
fn unnecessary_assert() {
    // LuaLS's disabled positive case and its negatives (test/diagnostics/unnecessary-assert.lua).
    let text = "assert(true)\nassert({})\nassert('x')\nlocal a\nassert(a)\n---@type boolean\nlocal b\nassert(b)\n---@type number?\nlocal f\nassert(f)\nassert(false)\nassert(nil and 5)\n---@type string\nlocal s = g()\nassert(s, 'msg')\nlocal lower = assert(string.lower)\nassert(function() end)\n---@type string[]\nlocal arr = {}\nassert(arr[1])\nassert(tostring)\nlocal t = { f = print }\nassert(t.f)\n";
    assert!(only(&codes(text), "unnecessary-assert").is_empty(), "off by default");
    let mut config = LintConfig::default();
    config.severity.insert("unnecessary-assert".into(), Some(Severity::Warning));
    let ctx = Context { config: &config, ..Context::default() };
    let p = parse(text);
    let n = names(&p, text);
    let lines: Vec<usize> =
        lint(&p, text, &n, &ctx).into_iter().filter(|d| d.code == "unnecessary-assert").map(|d| text[..usize::from(d.range.start())].matches('\n').count() + 1).collect();
    // Narrower than LuaLS: a typed local (16), an index (21) and a field of a
    // local table (24) are left alone.
    assert_eq!(lines, [1, 2, 3, 17, 18, 22]);

    // Library functions only where they surely exist: not other Lua versions'
    // (nil in CfxLua), not a native of the other side, not one the file replaces.
    let text = "assert(unpack)\nassert(loadstring)\nassert(setfenv)\nassert(GetNumPlayerIdentifiers)\nassert(PlayerPedId)\nassert(GetHashKey)\nassert(table.unpack)\n";
    let side_lines = |side: FileSide, text: &str| -> Vec<usize> {
        let ctx = Context { config: &config, side, ..Context::default() };
        let p = parse(text);
        let n = names(&p, text);
        lint(&p, text, &n, &ctx)
            .into_iter()
            .filter(|d| d.code == "unnecessary-assert")
            .map(|d| text[..usize::from(d.range.start())].matches('\n').count() + 1)
            .collect()
    };
    assert_eq!(side_lines(FileSide::Client, text), [5, 6, 7]);
    assert_eq!(side_lines(FileSide::Server, text), [4, 6, 7]);
    assert_eq!(side_lines(FileSide::Shared, text), [6, 7]);
    assert_eq!(side_lines(FileSide::Client, "GetHashKey = nil\nassert(GetHashKey)\n"), Vec::<usize>::new());
}

#[test]
fn codestyle_check_first_per_start() {
    let mut config = LintConfig::default();
    config.severity.insert("codestyle-check".into(), Some(Severity::Warning));
    let mut style = cfx_lua_fmt::Style::default();
    style.apply(&[("call_arg_parentheses".to_string(), "always".to_string()), ("quote_style".to_string(), "double".to_string())].into_iter().collect());
    let text = "local m = require 'x'\n";
    let p = parse(text);
    let got: Vec<String> = codestyle(&p, text, &style, &config).into_iter().map(|l| l.message).collect();
    // LuaLS keeps the first result of a code at one place: "expected (" hides
    // the quote and ")" findings on the same string; the space before it stays.
    assert_eq!(got, ["expected (", "unnecessary whitespace "]);
}

#[test]
fn missing_fields_child_overrides_parent() {
    // The child's own optional `image` hides the parent's required one.
    let f = codes("---@class Base
---@field id string
---@field image string
---@class Item: Base
---@field image? string
---@type Item
local a = { id = 'x' }
---@type Base
local b = { id = 'y' }
print(a, b)
");
    assert_eq!(only(&f, "missing-fields"), ["{ id = 'y' }"], "{f:?}");
}

#[test]
fn or_false_then_or_default() {
    // `c and v or false` is `v|false`; `s or d()` then drops the `false` (LuaLS gives no warning).
    let f = codes("---@class St
---@field a integer
---@return St
local function def() return { a = 1 } end
---@param s St
local function use(s) print(s) end
local function h(v)
    local s = type(v) == 'table' and v or false
    use(s or def())
end
h({})
");
    assert!(only(&f, "param-type-mismatch").is_empty(), "{f:?}");
}

#[test]
fn concat_in_loop_skips_locals_of_the_loop() {
    let f = codes("local out = {}\nfor i = 1, 3 do\n    local line = tostring(i)\n    if i > 1 then\n        line = line .. ' (b)'\n    end\n    out[#out + 1] = line\n    for j = 1, 2 do line = line .. j end\nend\nprint(out)\n");
    // The inner loop still grows `line` on every one of its iterations.
    assert_eq!(only(&f, "perf/concat-in-loop").len(), 1, "{f:?}");
    assert!(has(&f, "perf/concat-in-loop", "line = line .. j"));
}

#[test]
fn type_tests_narrow_when_false() {
    let f = codes("---@param s string
local function S(s) return s end
---@param x string|number
local function f1(x)
    if type(x) ~= 'string' then return nil end
    return S(x)
end
---@param x string|number
local function f2(x)
    if type(x) ~= 'string' or x == '' then return nil end
    return S(x)
end
---@param x string|string[]
local function f3(x)
    if type(x) == 'table' then return nil else return S(x) end
end
---@param x string|number
local function f4(x)
    if type(x) == 'number' then return nil end
    return S(x)
end
---@param x string|number
local function f5(x)
    if type(x) ~= 'number' then return S(x) end
end
---@param x string|number
local function f6(x)
    if type(x) == 'string' then return nil end
    return S(x)
end
print(f1, f2, f3, f4, f5, f6)
");
    // Only `f6`: there `x` is a number.
    assert_eq!(only(&f, "param-type-mismatch").len(), 1, "{f:?}");
}

#[test]
fn type_test_with_a_write_in_the_branch_narrows_after_it() {
    let f = codes("---@param n integer
local function I(n) return n end
---@param s string
---@return integer
local function H(s) return #s end
---@param m integer|string
local function f1(m)
    if type(m) == 'string' then m = H(m) end
    return I(m)
end
---@param m integer|string
local function f2(m)
    if type(m) == 'string' then
        if m == '' then m = H(m) end
    end
    return I(m)
end
---@param m integer|string
local function f3(m)
    if type(m) ~= 'number' then m = H(m) end
    return I(m)
end
print(f1, f2, f3)
");
    // Only `f2`: its write is not on every path of the branch.
    assert_eq!(only(&f, "param-type-mismatch").len(), 1, "{f:?}");
}

#[test]
fn and_value_that_may_be_nil_does_not_fit_a_parameter() {
    // LuaLS (nil checked): `n and tostring(n)` is `string?`, the parameter takes no nil.
    let config = crate::lint::LintConfig { weak_nil_check: Some(false), ..Default::default() };
    let ctx = Context { config: &config, ..Context::default() };
    let f = run("---@param x string|number
---@param n number?
local function f(x, n)
    x = n and tostring(n)
    return x
end
---@param x string?
---@param n number?
local function g(x, n)
    x = n and tostring(n)
    return x
end
print(f, g)
", &ctx);
    assert_eq!(only(&f, "cast-local-type"), ["x"], "{f:?}");
}

#[test]
fn doc_above_blank_line_is_not_bound() {
    // LuaLS: the `---@class` above the blank line does not type `Api`.
    let f = codes("---@class Res\n---@field ok boolean\n\nApi = {}\n\n---@param a string\n---@return boolean\nfunction Api.F(a) return a ~= '' end\n\n---@param name string\n---@param fn function\nlocal function Exp(name, fn) print(name, fn) end\nExp('F', Api.F)\nprint(Api.G)\n");
    assert!(only(&f, "param-type-mismatch").is_empty(), "{f:?}");
    assert!(only(&f, "undefined-field").is_empty(), "{f:?}");
}

#[test]
fn literal_tests_narrow_a_literal_union() {
    let f = codes("---@param p 'premium'|'verified'
local function plan(p) return p end
---@param n integer
local function num(n) return n end
---@param badge 'government'|'business'|'verified'|'premium'
---@param id integer|string
local function set(badge, id)
    if badge == 'verified' or badge == 'premium' then plan(badge) end
    if id == 1 then num(id) end
    if badge == 'government' then plan(badge) end
end
set('verified', 1)
");
    // Only the last call: `'government'` is no plan.
    assert_eq!(only(&f, "param-type-mismatch").len(), 1, "{f:?}");
}
