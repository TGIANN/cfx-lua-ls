//! Times inlay hints over one byte range of a file, with and without the
//! workspace: `cargo run --release -p cfx-lua-ide --example inlay_probe -- <folder> <file.lua> <start> <end>`.
//! `inlay_probe -- <folder> --labels` counts every type label of the folder's
//! files and prints the most common ones and those holding `unknown`, `any` or `<`.

use cfx_lua_ide::workspace::Workspace;
use cfx_lua_ide::{FileCtx, InlayKind, TextRange, TextSize, inlay_hints, is_escrow, names, parse};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn collect(path: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(path) else { return };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if !matches!(entry.file_name().to_str(), Some("node_modules" | ".git" | "stream")) {
                collect(&p, out);
            }
        } else if p.extension().is_some_and(|e| e == "lua") {
            out.push(p);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let ws = Workspace::scan(&[PathBuf::from(&args[0])]);
    if args[1] == "--labels" {
        let mut files = Vec::new();
        collect(Path::new(&args[0]), &mut files);
        let mut counts: HashMap<String, usize> = HashMap::new();
        let mut odd = Vec::new();
        for path in &files {
            let Ok(text) = std::fs::read_to_string(path) else { continue };
            if is_escrow(&text) {
                continue;
            }
            let p = parse(&text);
            let n = names(&p, &text);
            let info = ws.info(path);
            let cx = FileCtx { game: info.game, side: info.side, ws: Some((&ws, path.as_path())) };
            let all = TextRange::up_to(TextSize::of(&text));
            for h in inlay_hints(&p, &n, &text, all, cx).into_iter().filter(|h| h.kind == InlayKind::Type) {
                let l = h.label.trim_start_matches(": ").to_string();
                if (l.contains("unknown") || l.contains("any") || l.contains('<')) && odd.len() < 40 {
                    odd.push(format!("{}:{} {l}", path.display(), u32::from(h.offset)));
                }
                *counts.entry(l).or_insert(0) += 1;
            }
        }
        let mut v: Vec<(String, usize)> = counts.into_iter().collect();
        v.sort_by_key(|x| std::cmp::Reverse(x.1));
        println!("{} type labels, {} distinct", v.iter().map(|x| x.1).sum::<usize>(), v.len());
        for (l, c) in v.iter().take(40) {
            println!("{c:6} {l}");
        }
        for o in odd {
            println!("odd: {o}");
        }
        return;
    }
    let path = PathBuf::from(&args[1]);
    let text = String::from_utf8_lossy(&std::fs::read(&path).expect("read the file")).into_owned();
    let range = TextRange::new(TextSize::from(args[2].parse::<u32>().expect("start")), TextSize::from(args[3].parse::<u32>().expect("end")));
    let p = parse(&text);
    let n = names(&p, &text);
    let info = ws.info(&path);
    for (label, cx) in [
        ("workspace", FileCtx { game: info.game, side: info.side, ws: Some((&ws, path.as_path())) }),
        ("file only", FileCtx { game: info.game, side: info.side, ws: None }),
    ] {
        for round in 0..2 {
            let t = Instant::now();
            let h = inlay_hints(&p, &n, &text, range, cx);
            let e = t.elapsed();
            let types = h.iter().filter(|h| h.kind == InlayKind::Type).count();
            println!("{label} round {round}: {} hints ({types} types) in {e:?}", h.len());
        }
    }
}
