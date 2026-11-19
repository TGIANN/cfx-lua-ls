//! The server end to end over lsp-server's in-memory connection.

use super::serve;
use lsp_server::{Connection, Message, Notification, Request, RequestId};
use serde_json::{Value, json};
use std::time::Duration;

struct Client {
    conn: Connection,
    next_id: i32,
}

impl Client {
    fn start(init_capabilities: Value) -> (Client, std::thread::JoinHandle<()>) {
        Client::start_with(json!({ "capabilities": init_capabilities }))
    }

    fn start_with(init_params: Value) -> (Client, std::thread::JoinHandle<()>) {
        let (server, client) = Connection::memory();
        let handle = std::thread::spawn(move || serve(&server).expect("server"));
        let mut c = Client { conn: client, next_id: 0 };
        let init = c.request("initialize", init_params);
        assert!(init["capabilities"]["textDocumentSync"].is_object(), "{init}");
        c.notify("initialized", json!({}));
        (c, handle)
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.try_request(method, params).expect("a successful response")
    }

    /// The result, or the error code.
    fn try_request(&mut self, method: &str, params: Value) -> Result<Value, i32> {
        self.next_id += 1;
        let id = RequestId::from(self.next_id);
        self.conn.sender.send(Message::Request(Request::new(id.clone(), method.into(), params))).unwrap();
        loop {
            match self.recv() {
                Message::Response(r) if r.id == id => return r.response_result.map_err(|e| e.code),
                _ => continue,
            }
        }
    }

    fn notify(&self, method: &str, params: Value) {
        self.conn.sender.send(Message::Notification(Notification::new(method.into(), params))).unwrap();
    }

    fn recv(&self) -> Message {
        self.conn.receiver.recv_timeout(Duration::from_secs(5)).expect("a message from the server")
    }

    /// The next `publishDiagnostics` notification's params.
    fn diagnostics(&self) -> Value {
        loop {
            if let Message::Notification(n) = self.recv()
                && n.method == "textDocument/publishDiagnostics"
            {
                return n.params;
            }
        }
    }

    fn shutdown(mut self, handle: std::thread::JoinHandle<()>) {
        self.request("shutdown", Value::Null);
        self.notify("exit", Value::Null);
        handle.join().unwrap();
    }
}

const URI: &str = "file:///c%3A/res/client.lua";

#[test]
fn open_change_close_publish_diagnostics() {
    let (mut c, handle) = Client::start(json!({}));
    c.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": URI, "languageId": "lua", "version": 1, "text": "local 😀 = 1\nlocal x = 2\n" } }),
    );
    let d = c.diagnostics();
    assert_eq!(d["version"], 1);
    let first = &d["diagnostics"][0];
    assert_eq!(first["source"], "Lua Syntax Check.");
    assert_eq!(first["severity"], 1);
    assert!(first["code"].is_string(), "{first}");

    // Replace the emoji (2 UTF-16 units) with a name: the file becomes valid.
    c.notify(
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": URI, "version": 2 },
            "contentChanges": [{ "range": { "start": { "line": 0, "character": 6 }, "end": { "line": 0, "character": 8 } }, "text": "y" }]
        }),
    );
    let d = c.diagnostics();
    assert_eq!(d["version"], 2);
    // No syntax error left; the unused locals are lint hints.
    let found = d["diagnostics"].as_array().unwrap();
    assert!(found.iter().all(|x| x["source"] == "cfx-lua-ls" && x["code"] == "unused-local"), "{d}");

    // Unknown requests get an error, not silence.
    let unknown = c.try_request("textDocument/unknownThing", json!({}));
    assert_eq!(unknown, Err(lsp_server::ErrorCode::MethodNotFound as i32));

    c.notify("textDocument/didClose", json!({ "textDocument": { "uri": URI } }));
    let d = c.diagnostics();
    assert_eq!(d["diagnostics"], json!([]));
    c.shutdown(handle);
}

#[test]
fn utf8_positions_when_the_client_offers_them() {
    let (mut c, handle) = Client::start(json!({ "general": { "positionEncodings": ["utf-8", "utf-16"] } }));
    c.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": URI, "languageId": "lua", "version": 1, "text": "x = 'ç' + \n" } }),
    );
    let d = c.diagnostics();
    // The error sits after `+` at the end of line 0: byte column 10 (ç is 2 bytes).
    assert_eq!(d["diagnostics"][0]["range"]["start"]["character"], 10, "{d}");
    c.request("shutdown", Value::Null);
    c.notify("exit", Value::Null);
    handle.join().unwrap();
}

#[test]
fn escrow_documents_get_no_diagnostics() {
    let (c, handle) = Client::start(json!({}));
    c.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": URI, "languageId": "lua", "version": 1, "text": "FXAP\u{1}\u{7f}((((" } }),
    );
    assert_eq!(c.diagnostics()["diagnostics"], json!([]));
    c.shutdown(handle);
}

#[test]
fn document_symbols_nested_with_utf16_ranges() {
    let (mut c, handle) = Client::start(json!({}));
    let text = "local function f(a)\n    local s = 'ç'\nend\nlocal function\n";
    c.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": URI, "languageId": "lua", "version": 1, "text": text } }),
    );
    c.diagnostics();
    let syms = c.request("textDocument/documentSymbol", json!({ "textDocument": { "uri": URI } }));
    let f = &syms[0];
    assert_eq!(
        (f["name"].as_str(), f["kind"].as_u64(), f["detail"].as_str()),
        (Some("f"), Some(12), Some("function (a)"))
    );
    assert_eq!(
        f["selectionRange"],
        json!({ "start": { "line": 0, "character": 15 }, "end": { "line": 0, "character": 16 } })
    );
    let local = &f["children"][1];
    assert_eq!((local["name"].as_str(), local["detail"].as_str()), (Some("s"), Some("\"ç\"")));
    // `s = 'ç'` is 7 UTF-16 units (8 bytes) from column 10.
    assert_eq!(local["range"]["end"], json!({ "line": 1, "character": 17 }));
    // A nameless function gets a space, as in LuaLS.
    assert_eq!(syms[1]["name"], " ");

    // Not opened: null, not an error.
    let other =
        c.request("textDocument/documentSymbol", json!({ "textDocument": { "uri": "file:///c%3A/other.lua" } }));
    assert_eq!(other, Value::Null);
    c.shutdown(handle);
}

#[test]
fn document_symbols_at_the_nesting_limit() {
    // Tables, `if` blocks and functions each nest one symbol per level, up to the
    // parser's limit; converting and sending them must not overflow the stack.
    let (mut c, handle) = Client::start(json!({}));
    let texts = [
        format!("local t = {}{}", "{ a = ".repeat(250), "1 }".repeat(250)),
        format!("{}{}", "if x then
".repeat(250), "end
".repeat(250)),
        format!("f({}{})", "function() g(".repeat(250), ") end".repeat(250)),
    ];
    for (version, text) in texts.into_iter().enumerate() {
        c.notify(
            "textDocument/didOpen",
            json!({ "textDocument": { "uri": URI, "languageId": "lua", "version": version, "text": text } }),
        );
        c.diagnostics();
        let syms = c.request("textDocument/documentSymbol", json!({ "textDocument": { "uri": URI } }));
        let mut depth = 0;
        let mut node = &syms[0];
        while let Some(child) = node["children"].get(0) {
            node = child;
            depth += 1;
        }
        assert!(depth >= 90, "case {version}: depth {depth}");
    }
    c.shutdown(handle);
}

#[test]
fn escrow_documents_get_no_symbols() {
    let (mut c, handle) = Client::start(json!({}));
    c.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": URI, "languageId": "lua", "version": 1, "text": "FXAP\u{1}local x = 1" } }),
    );
    c.diagnostics();
    assert_eq!(c.request("textDocument/documentSymbol", json!({ "textDocument": { "uri": URI } })), Value::Null);
    c.shutdown(handle);
}

#[test]
fn folding_ranges_by_line() {
    let (mut c, handle) = Client::start(json!({}));
    let text = "local function f()\r\n    x()\r\nend\r\n--[[\r\nc\r\n]]\r\n";
    c.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": URI, "languageId": "lua", "version": 1, "text": text } }),
    );
    c.diagnostics();
    let folds = c.request("textDocument/foldingRange", json!({ "textDocument": { "uri": URI } }));
    assert_eq!(
        folds,
        json!([
            { "startLine": 0, "endLine": 1, "kind": "region" },
            { "startLine": 3, "endLine": 4, "kind": "comment" },
        ])
    );
    let other = c.request("textDocument/foldingRange", json!({ "textDocument": { "uri": "file:///c%3A/other.lua" } }));
    assert_eq!(other, Value::Null);
    c.shutdown(handle);
}

#[test]
fn selection_ranges_per_position() {
    let (mut c, handle) = Client::start(json!({}));
    let text = "local s = 'çx' .. y\n";
    c.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": URI, "languageId": "lua", "version": 1, "text": text } }),
    );
    c.diagnostics();
    let res = c.request(
        "textDocument/selectionRange",
        json!({ "textDocument": { "uri": URI }, "positions": [{ "line": 0, "character": 12 }, { "line": 0, "character": 18 }] }),
    );
    let ranges = |mut s: &Value| {
        let mut out = Vec::new();
        while !s.is_null() {
            out.push((s["range"]["start"]["character"].as_u64().unwrap(), s["range"]["end"]["character"].as_u64().unwrap()));
            s = &s["parent"];
        }
        out
    };
    // UTF-16 columns: `çx` inside the quotes, the string, `'çx' .. y`, the statement, the file.
    assert_eq!(ranges(&res[0]), [(11, 13), (10, 14), (10, 19), (0, 19), (0, 0)]);
    assert_eq!(ranges(&res[1])[0], (18, 19));
    c.shutdown(handle);
}

#[test]
fn semantic_tokens_full() {
    let (server, client) = Connection::memory();
    let handle = std::thread::spawn(move || serve(&server).expect("server"));
    let mut c = Client { conn: client, next_id: 0 };
    let init = c.request("initialize", json!({ "capabilities": {} }));
    let legend = &init["capabilities"]["semanticTokensProvider"]["legend"];
    assert_eq!(legend["tokenTypes"][15], "keyword");
    assert_eq!(legend["tokenModifiers"][0], "declaration");
    c.notify("initialized", json!({}));
    c.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": URI, "languageId": "lua", "version": 1, "text": "local x = 1\n  return x" } }),
    );
    c.diagnostics();
    let res = c.request("textDocument/semanticTokens/full", json!({ "textDocument": { "uri": URI } }));
    // local (keyword, declaration), x (variable, declaration), 1 (number, static), return (keyword).
    assert_eq!(res["data"], json!([0, 0, 5, 15, 1, 0, 6, 1, 8, 1, 0, 4, 1, 19, 8, 1, 2, 6, 15, 0]));
    c.shutdown(handle);
}

#[test]
fn selection_in_unterminated_strings() {
    // The contents step must not drop a closing delimiter that is not there: the
    // range would end inside `ç`.
    let (mut c, handle) = Client::start(json!({}));
    for (version, text) in ["x = 'ç", "x = `ç", "x = [[ç", r"x = 'a\'"].into_iter().enumerate() {
        c.notify(
            "textDocument/didOpen",
            json!({ "textDocument": { "uri": URI, "languageId": "lua", "version": version, "text": text } }),
        );
        c.diagnostics();
        let res = c.request(
            "textDocument/selectionRange",
            json!({ "textDocument": { "uri": URI }, "positions": [{ "line": 0, "character": 5 }] }),
        );
        assert!(res[0]["range"].is_object(), "{text:?}: {res}");
    }
    c.shutdown(handle);
}

#[test]
fn definition_references_highlight_rename() {
    let (mut c, handle) = Client::start(json!({}));
    open(&c, URI, "local x = 1\nx = x + 1\nprint(x)\n");
    let pos = |line: u32, character: u32| json!({ "textDocument": { "uri": URI }, "position": { "line": line, "character": character } });
    let range = |l: u32, s: u32, e: u32| json!({ "start": { "line": l, "character": s }, "end": { "line": l, "character": e } });
    let def = c.request("textDocument/definition", pos(2, 6));
    assert_eq!(def, json!([{ "uri": URI, "range": range(0, 6, 7) }, { "uri": URI, "range": range(1, 0, 1) }]));
    let mut refs = pos(2, 6);
    refs["context"] = json!({ "includeDeclaration": false });
    let refs = c.request("textDocument/references", refs);
    assert_eq!(refs.as_array().unwrap().len(), 2, "{refs}");
    let hl = c.request("textDocument/documentHighlight", pos(0, 6));
    assert_eq!(hl[0], json!({ "range": range(0, 6, 7), "kind": 3 }));
    assert_eq!(hl.as_array().unwrap().len(), 4);
    let prep = c.request("textDocument/prepareRename", pos(1, 4));
    assert_eq!(prep, range(1, 4, 5));
    let mut rename = pos(1, 4);
    rename["newName"] = json!("count");
    let edit = c.request("textDocument/rename", rename.clone());
    assert_eq!(edit["changes"][URI].as_array().unwrap().len(), 4);
    rename["newName"] = json!("end");
    assert_eq!(c.try_request("textDocument/rename", rename), Err(lsp_server::ErrorCode::RequestFailed as i32));
    // Keywords highlight their partners.
    open(&c, URI, "if a then\nelse\nend\n");
    let hl = c.request("textDocument/documentHighlight", pos(1, 1));
    assert_eq!(hl.as_array().unwrap().len(), 4, "{hl}");
    c.shutdown(handle);
}

fn open(c: &Client, uri: &str, text: &str) {
    c.notify("textDocument/didOpen", json!({ "textDocument": { "uri": uri, "languageId": "lua", "version": 1, "text": text } }));
    c.diagnostics();
}

const OPTIONS: fn() -> Value = || json!({ "tabSize": 2, "insertSpaces": true });

#[test]
fn formatting_the_whole_document() {
    let (mut c, handle) = Client::start(json!({}));
    open(&c, URI, "if a then\nb()\nend\n");
    let res = c.request("textDocument/formatting", json!({ "textDocument": { "uri": URI }, "options": OPTIONS() }));
    assert_eq!(
        res,
        json!([{ "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 3, "character": 0 } }, "newText": "if a then\n  b()\nend\n" }])
    );
    // Already formatted, escrow or broken: no edit.
    for text in ["local a = 1\n", "FXAP\u{1}local  a=1", "local  a = (\n"] {
        open(&c, URI, text);
        let res = c.request("textDocument/formatting", json!({ "textDocument": { "uri": URI }, "options": OPTIONS() }));
        assert_eq!(res, Value::Null, "{text:?}");
    }
    c.shutdown(handle);
}

#[test]
fn formatting_a_range() {
    let (mut c, handle) = Client::start(json!({}));
    open(&c, URI, "local  a=1\nlocal  b=2\nlocal  c=3");
    let range = |line: u32| json!({ "start": { "line": line, "character": 3 }, "end": { "line": line, "character": 5 } });
    let res = c.request(
        "textDocument/rangeFormatting",
        json!({ "textDocument": { "uri": URI }, "range": range(1), "options": OPTIONS() }),
    );
    assert_eq!(
        res,
        json!([{ "range": { "start": { "line": 1, "character": 0 }, "end": { "line": 2, "character": 0 } }, "newText": "local b = 2\n" }])
    );
    // The last line has no line break after it: the edit ends at the end of the text.
    let res = c.request(
        "textDocument/rangeFormatting",
        json!({ "textDocument": { "uri": URI }, "range": range(2), "options": OPTIONS() }),
    );
    assert_eq!(res[0]["range"]["end"], json!({ "line": 2, "character": 10 }), "{res}");
    c.shutdown(handle);
}

#[test]
fn format_settings_from_the_client() {
    let (mut c, handle) = Client::start(json!({}));
    open(&c, URI, "x = \"a\"\n");
    let format = |c: &mut Client| {
        c.request("textDocument/formatting", json!({ "textDocument": { "uri": URI }, "options": OPTIONS() }))
    };
    assert_eq!(format(&mut c), Value::Null);
    c.notify(
        "workspace/didChangeConfiguration",
        json!({ "settings": { "Lua": { "format": { "defaultConfig": { "quote_style": "single" } } } } }),
    );
    assert_eq!(format(&mut c)[0]["newText"], "x = 'a'\n");
    c.notify("workspace/didChangeConfiguration", json!({ "settings": { "Lua": { "format": { "enable": false } } } }));
    open(&c, URI, "x  =  1\n");
    assert_eq!(format(&mut c), Value::Null);
    c.shutdown(handle);
}

#[test]
fn editorconfig_up_to_the_workspace_folder() {
    let root = std::env::temp_dir().join(format!("cfx-lua-ls-editorconfig-{}", std::process::id()));
    let res = root.join("res").join("sub");
    std::fs::create_dir_all(&res).unwrap();
    std::fs::write(root.join(".editorconfig"), "[*.lua]\nquote_style = single\n").unwrap();
    let to_uri = |p: &std::path::Path| {
        let s = p.to_string_lossy().replace('\\', "/").replace(' ', "%20");
        if s.starts_with('/') { format!("file://{s}") } else { format!("file:///{s}") }
    };
    let file = to_uri(&res.join("a.lua"));
    let workspace = to_uri(&root);
    let (mut c, handle) =
        Client::start_with(json!({ "capabilities": {}, "workspaceFolders": [{ "uri": workspace, "name": "w" }] }));
    open(&c, &file, "x = \"a\"\n");
    let res_ = c.request("textDocument/formatting", json!({ "textDocument": { "uri": file }, "options": OPTIONS() }));
    assert_eq!(res_[0]["newText"], "x = 'a'\n", "{res_}");
    c.shutdown(handle);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn hover_completion_signature_help() {
    let (mut c, handle) = Client::start(json!({}));
    open(&c, URI, "local p = GetEntityCoords(PlayerPedId())\nSetEntityCoords(p, \nlocal s = string.\n");
    let h = c.request("textDocument/hover", json!({ "textDocument": { "uri": URI }, "position": { "line": 0, "character": 14 } }));
    assert!(h["contents"]["value"].as_str().unwrap().contains("function GetEntityCoords("), "{h}");
    let list = c.request("textDocument/completion", json!({ "textDocument": { "uri": URI }, "position": { "line": 2, "character": 17 } }));
    let labels: Vec<&str> = list["items"].as_array().unwrap().iter().map(|i| i["label"].as_str().unwrap()).collect();
    assert!(labels.contains(&"format") && labels.contains(&"gsub"), "{labels:?}");
    let s = c.request("textDocument/signatureHelp", json!({ "textDocument": { "uri": URI }, "position": { "line": 1, "character": 19 } }));
    assert_eq!(s["activeParameter"], 1, "{s}");
    assert!(s["signatures"][0]["label"].as_str().unwrap().starts_with("SetEntityCoords("));
    c.shutdown(handle);
}

#[test]
fn inlay_hints_when_enabled() {
    let (mut c, handle) = Client::start(json!({ "workspace": { "inlayHint": { "refreshSupport": true } } }));
    open(&c, URI, "local c = GetEntityCoords(PlayerPedId())\nSetEntityCoords(1, 2.0, 3.0, 4.0)\n");
    let all = json!({ "textDocument": { "uri": URI }, "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 2, "character": 0 } } });
    assert_eq!(c.request("textDocument/inlayHint", all.clone()), json!([]));
    c.notify("workspace/didChangeConfiguration", json!({ "settings": { "Lua": { "hint": { "enable": true } } } }));
    // Turning hints on asks the client to fetch them again.
    loop {
        if let Message::Request(r) = c.recv() {
            assert_eq!(r.method, "workspace/inlayHint/refresh");
            break;
        }
    }
    let h = c.request("textDocument/inlayHint", all.clone());
    let labels: Vec<&str> = h.as_array().unwrap().iter().map(|x| x["label"].as_str().unwrap()).collect();
    assert_eq!(labels, [": vector3", "entity:", "entity:", "xPos:", "yPos:", "zPos:"], "{h}");
    assert_eq!(h[0]["position"], json!({ "line": 0, "character": 7 }), "{h}");
    c.notify("workspace/didChangeConfiguration", json!({ "settings": { "Lua": { "hint": { "enable": true, "paramName": "Disable" } } } }));
    let h = c.request("textDocument/inlayHint", all);
    assert_eq!(h.as_array().unwrap().len(), 1, "{h}");
    c.shutdown(handle);
}

#[test]
fn lint_over_resources_with_quick_fixes() {
    let root = std::env::temp_dir().join(format!("cfx-lua-ls-lint-{}", std::process::id()));
    let res = root.join("app");
    std::fs::create_dir_all(&res).unwrap();
    std::fs::write(
        res.join("fxmanifest.lua"),
        "shared_script '@ox_lib/init.lua'\nshared_script 'config.lua'\nclient_script 'client.lua'\nserver_script 'server.lua'\n",
    )
    .unwrap();
    std::fs::write(res.join("config.lua"), "Config = {}\n").unwrap();
    std::fs::write(res.join("server.lua"), "ServerOnly = 1\n").unwrap();
    std::fs::write(root.join(".luarc.json"), "{ \"diagnostics.globals\": [\"Known\"] }").unwrap();
    let to_uri = |p: &std::path::Path| {
        let s = p.to_string_lossy().replace('\\', "/").replace(' ', "%20");
        if s.starts_with('/') { format!("file://{s}") } else { format!("file:///{s}") }
    };
    let file = to_uri(&res.join("client.lua"));
    let (mut c, handle) =
        Client::start_with(json!({ "capabilities": {}, "workspaceFolders": [{ "uri": to_uri(&root), "name": "w" }] }));
    let text = "print(Config, Known, ServerOnly, lib)\nlocal ped = PlayerPedId()\nprint(ped)\n";
    c.notify("textDocument/didOpen", json!({ "textDocument": { "uri": file, "languageId": "lua", "version": 1, "text": text } }));
    let d = c.diagnostics();
    let codes: Vec<(String, u64)> = d["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| (x["code"].as_str().unwrap().to_string(), x["range"]["start"]["character"].as_u64().unwrap()))
        .collect();
    // `Config` is shared, `Known` is configured, `lib` comes from ox_lib; `ServerOnly` is
    // on the other side.
    assert_eq!(codes, [("undefined-global".to_string(), 21), ("ox/cache-ped".to_string(), 12)], "{d}");
    let actions = c.request(
        "textDocument/codeAction",
        json!({ "textDocument": { "uri": file }, "range": { "start": { "line": 1, "character": 14 }, "end": { "line": 1, "character": 14 } }, "context": { "diagnostics": [] } }),
    );
    assert_eq!(actions[0]["title"], "Use cache.ped", "{actions}");
    assert_eq!(actions[0]["edit"]["changes"][&file][0]["newText"], "cache.ped");
    assert_eq!(actions[1]["title"], "Disable ox/cache-ped for this line");
    assert_eq!(actions[1]["edit"]["changes"][&file][0]["newText"], "---@diagnostic disable-next-line: ox/cache-ped\n");
    c.shutdown(handle);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn event_names_across_resources() {
    let root = std::env::temp_dir().join(format!("cfx-lua-ls-events-{}", std::process::id()));
    let res = root.join("app");
    std::fs::create_dir_all(&res).unwrap();
    std::fs::write(res.join("fxmanifest.lua"), "client_script 'client.lua'\nserver_script 'server.lua'\n").unwrap();
    std::fs::write(res.join("server.lua"), "RegisterNetEvent('app:buy', function() end)\nRegisterNetEvent('app:sell', function() end)\n").unwrap();
    let to_uri = |p: &std::path::Path| {
        let s = p.to_string_lossy().replace('\\', "/").replace(' ', "%20");
        if s.starts_with('/') { format!("file://{s}") } else { format!("file:///{s}") }
    };
    let file = to_uri(&res.join("client.lua"));
    let (mut c, handle) =
        Client::start_with(json!({ "capabilities": {}, "workspaceFolders": [{ "uri": to_uri(&root), "name": "w" }] }));
    let text = "TriggerServerEvent('app:b')\nTriggerServerEvent('app:sell')\n";
    c.notify("textDocument/didOpen", json!({ "textDocument": { "uri": file, "languageId": "lua", "version": 1, "text": text } }));
    let items = c.request("textDocument/completion", json!({ "textDocument": { "uri": file }, "position": { "line": 0, "character": 25 } }));
    let items = items["items"].as_array().unwrap();
    assert_eq!(items[0]["label"], "app:buy", "{items:?}");
    assert!(items.iter().all(|i| i["label"] != "app:sell"), "{items:?}");
    // The edit replaces the whole string's content.
    assert_eq!(items[0]["textEdit"]["range"]["start"]["character"], 20);
    assert_eq!(items[0]["textEdit"]["range"]["end"]["character"], 25);
    let def = c.request("textDocument/definition", json!({ "textDocument": { "uri": file }, "position": { "line": 1, "character": 24 } }));
    let def = def.as_array().unwrap();
    assert_eq!(def.len(), 1, "{def:?}");
    assert!(def[0]["uri"].as_str().unwrap().ends_with("app/server.lua"), "{def:?}");
    assert_eq!(def[0]["range"]["start"]["line"], 1);
    c.shutdown(handle);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn class_fields_and_names_across_files() {
    let root = std::env::temp_dir().join(format!("cfx-lua-ls-typed-{}", std::process::id()));
    let res = root.join("app");
    std::fs::create_dir_all(&res).unwrap();
    std::fs::write(res.join("fxmanifest.lua"), "shared_script 'config.lua'\nclient_script 'client.lua'\n").unwrap();
    std::fs::write(res.join("config.lua"), "---@class AppConfig\n---@field speed number\nConfig = {}\n").unwrap();
    // On disk the client is empty: the open document's text is what counts.
    std::fs::write(res.join("client.lua"), "").unwrap();
    let to_uri = |p: &std::path::Path| {
        let s = p.to_string_lossy().replace('\\', "/").replace(' ', "%20");
        if s.starts_with('/') { format!("file://{s}") } else { format!("file:///{s}") }
    };
    let file = to_uri(&res.join("client.lua"));
    let config = to_uri(&res.join("config.lua"));
    let (mut c, handle) =
        Client::start_with(json!({ "capabilities": {}, "workspaceFolders": [{ "uri": to_uri(&root), "name": "w" }] }));
    let text = "---@type AppConfig\nlocal cfg = Config\nprint(cfg.speed)\n";
    c.notify("textDocument/didOpen", json!({ "textDocument": { "uri": file, "languageId": "lua", "version": 1, "text": text } }));
    let _ = c.diagnostics();
    let at = |line: u32, character: u32| json!({ "textDocument": { "uri": file }, "position": { "line": line, "character": character } });
    let mut refs = at(2, 11);
    refs["context"] = json!({ "includeDeclaration": true });
    let refs = c.request("textDocument/references", refs);
    let uris: Vec<&str> = refs.as_array().unwrap().iter().map(|l| l["uri"].as_str().unwrap()).collect();
    assert_eq!(uris.len(), 2, "{refs}");
    assert!(uris.contains(&config.as_str()) && uris.contains(&file.as_str()), "{refs}");
    let prep = c.request("textDocument/prepareRename", at(2, 11));
    assert_eq!(prep["start"]["character"], 10, "{prep}");
    let mut rename = at(2, 11);
    rename["newName"] = json!("pace");
    let edit = c.request("textDocument/rename", rename);
    assert_eq!(edit["changes"][&file][0]["range"]["start"]["character"], 10, "{edit}");
    assert_eq!(edit["changes"][&config][0]["range"]["start"]["line"], 1, "{edit}");
    // The class name, from its use in a doc comment.
    let mut rename = at(0, 12);
    rename["newName"] = json!("ClientConfig");
    let edit = c.request("textDocument/rename", rename);
    assert_eq!(edit["changes"][&config][0]["newText"], "ClientConfig", "{edit}");
    assert_eq!(edit["changes"][&file][0]["range"]["start"]["character"], 9, "{edit}");
    c.shutdown(handle);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn style_and_spell_checks_from_client_settings() {
    let root = std::env::temp_dir().join(format!("cfx-lua-ls-style-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let to_uri = |p: &std::path::Path| {
        let s = p.to_string_lossy().replace('\\', "/").replace(' ', "%20");
        if s.starts_with('/') { format!("file://{s}") } else { format!("file:///{s}") }
    };
    let file = to_uri(&root.join("client.lua"));
    let (mut c, handle) = Client::start_with(json!({
        "capabilities": {},
        "workspaceFolders": [{ "uri": to_uri(&root), "name": "w" }],
        "initializationOptions": { "spellAddWord": true },
    }));
    let any = json!({ "spell-check": "Any", "name-style-check": "Any", "codestyle-check": "Any" });
    c.notify("workspace/didChangeConfiguration", json!({ "settings": { "Lua": { "diagnostics": { "neededFileStatus": any } } } }));
    let text = "local playerNmae=1\nprint(playerNmae, \"abc\")\n";
    c.notify("textDocument/didOpen", json!({ "textDocument": { "uri": file, "languageId": "lua", "version": 1, "text": text } }));
    let codes = |d: &Value| -> Vec<(String, String)> {
        d["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| (x["code"].as_str().unwrap().to_string(), x["message"].as_str().unwrap().to_string()))
            .collect()
    };
    let d = codes(&c.diagnostics());
    for code in ["name-style-check", "spell-check", "codestyle-check"] {
        assert!(d.iter().any(|(c, _)| c == code), "{code}: {d:?}");
    }
    assert!(!d.iter().any(|(_, m)| m.contains("should be")), "{d:?}");
    let actions = c.request(
        "textDocument/codeAction",
        json!({ "textDocument": { "uri": file }, "range": { "start": { "line": 0, "character": 13 }, "end": { "line": 0, "character": 13 } }, "context": { "diagnostics": [] } }),
    );
    let titles: Vec<&str> = actions.as_array().unwrap().iter().map(|a| a["title"].as_str().unwrap()).collect();
    assert!(titles.contains(&"Add 'Nmae' to workspace dict"), "{titles:?}");
    assert!(titles.contains(&"Name"), "{titles:?}");
    // codestyle-check follows an `.editorconfig` written after the file opened.
    std::fs::write(root.join(".editorconfig"), "root = true\n\n[*.lua]\nquote_style = single\n").unwrap();
    c.notify(
        "workspace/didChangeWatchedFiles",
        json!({ "changes": [{ "uri": to_uri(&root.join(".editorconfig")), "type": 1 }] }),
    );
    let d = codes(&c.diagnostics());
    assert!(d.iter().any(|(c, m)| c == "codestyle-check" && m.starts_with("\" should be '")), "{d:?}");
    c.shutdown(handle);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn spell_add_word_only_for_clients_that_run_it() {
    let (mut c, handle) = Client::start(json!({}));
    c.notify(
        "workspace/didChangeConfiguration",
        json!({ "settings": { "Lua": { "diagnostics": { "neededFileStatus": { "spell-check": "Any" } } } } }),
    );
    open(&c, URI, "local playerNmae = 1\nprint(playerNmae)\n");
    let actions = c.request(
        "textDocument/codeAction",
        json!({ "textDocument": { "uri": URI }, "range": { "start": { "line": 0, "character": 13 }, "end": { "line": 0, "character": 13 } }, "context": { "diagnostics": [] } }),
    );
    let titles: Vec<&str> = actions.as_array().unwrap().iter().map(|a| a["title"].as_str().unwrap()).collect();
    assert!(titles.contains(&"Name") && !titles.iter().any(|t| t.starts_with("Add '")), "{titles:?}");
    c.shutdown(handle);
}
