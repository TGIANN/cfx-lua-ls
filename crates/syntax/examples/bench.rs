//! Parse speed on a folder, files read into memory first, one thread:
//! `cargo run --release -p cfx-lua-syntax --example bench -- <folder> [rounds]`

use std::path::Path;
use std::time::Instant;

fn collect(dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if path.is_dir() {
            if !matches!(name.to_str(), Some("node_modules" | ".git" | "stream")) {
                collect(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "lua")
            && let Ok(bytes) = std::fs::read(&path)
            && !bytes.starts_with(b"FXAP")
        {
            out.push(String::from_utf8_lossy(&bytes).into_owned());
        }
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().expect("usage: bench <folder> [rounds]");
    let rounds: usize = args.next().map_or(5, |r| r.parse().expect("rounds"));
    let mut files = Vec::new();
    collect(Path::new(&dir), &mut files);
    let bytes: usize = files.iter().map(String::len).sum();
    let mut best = f64::MAX;
    for _ in 0..rounds {
        let start = Instant::now();
        let mut errors = 0;
        for text in &files {
            errors += cfx_lua_syntax::parse(text).errors().len();
        }
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        best = best.min(ms);
        std::hint::black_box(errors);
    }
    println!("{} files, {:.1} MB, best of {rounds}: {best:.0} ms", files.len(), bytes as f64 / 1048576.0);
}
