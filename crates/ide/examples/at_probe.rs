//! Times completion, hover, signature help and definition at one offset:
//! `cargo run --release -p cfx-lua-ide --example at_probe -- <folder> <file.lua> <offset>`.

use cfx_lua_ide::workspace::Workspace;
use cfx_lua_ide::{FileCtx, TextSize, complete, hover, names, parse, signature_help};
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let ws = Workspace::scan(&[PathBuf::from(&args[0])]);
    let path = PathBuf::from(&args[1]);
    let text = String::from_utf8_lossy(&std::fs::read(&path).expect("read the file")).into_owned();
    let at = TextSize::from(args[2].parse::<u32>().expect("offset"));
    let p = parse(&text);
    let n = names(&p, &text);
    let info = ws.info(&path);
    let cx = FileCtx { game: info.game, side: info.side, ws: Some((&ws, path.as_path())) };
    let start = usize::from(at).saturating_sub(60);
    println!("…{}‸", &text[start..usize::from(at)]);
    for round in 0..2 {
        let t = Instant::now();
        let c = complete(&p, &n, &text, at, cx).map_or(0, |c| c.items.len());
        let tc = t.elapsed();
        let t = Instant::now();
        let h = hover(&p, &n, &text, at, cx).is_some();
        let th = t.elapsed();
        let t = Instant::now();
        let s = signature_help(&p, &n, &text, at, cx).is_some();
        let ts = t.elapsed();
        let t = Instant::now();
        let d = cfx_lua_ide::api_definition(&p, &n, &text, at, &ws, &path).is_some();
        let td = t.elapsed();
        println!("round {round}: complete {c} in {tc:?}, hover {h} in {th:?}, signature {s} in {ts:?}, definition {d} in {td:?}");
    }
}
