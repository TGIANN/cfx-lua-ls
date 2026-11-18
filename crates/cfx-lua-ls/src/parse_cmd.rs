//! `cfx-lua-ls parse`: parses every `.lua` file under the given paths, prints the
//! syntax errors and timing. Used to check the parser against real resources.

use rayon::prelude::*;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Folders that never hold resource Lua worth analysing.
const SKIP_DIRS: &[&str] = &["node_modules", ".git", "stream"];

fn collect_lua(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_file() {
        if path.extension().is_some_and(|e| e == "lua") {
            out.push(path.to_path_buf());
        }
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else { return };
    for entry in entries.flatten() {
        let p = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            if !SKIP_DIRS.iter().any(|s| entry.file_name() == *s) {
                collect_lua(&p, out);
            }
        } else if ft.is_file() && p.extension().is_some_and(|e| e == "lua") {
            out.push(p);
        }
    }
}

struct FileResult {
    path: PathBuf,
    bytes: usize,
    /// (line, column, message)
    errors: Vec<(usize, usize, String)>,
    lossless: bool,
}

pub(crate) fn parse_cmd(paths: &[String]) {
    let started = Instant::now();
    let mut files = Vec::new();
    for p in paths {
        if !Path::new(p).exists() {
            eprintln!("path not found: {p}");
            std::process::exit(2);
        }
        collect_lua(Path::new(p), &mut files);
    }
    if files.is_empty() {
        eprintln!("no .lua files found under: {}", paths.join(", "));
        std::process::exit(2);
    }
    let walk_time = started.elapsed();

    let pool = rayon::ThreadPoolBuilder::new().stack_size(16 << 20).build().unwrap();
    let parse_start = Instant::now();
    let results: Vec<FileResult> = pool.install(|| {
        files
            .par_iter()
            .filter_map(|path| {
                let bytes = std::fs::read(path).unwrap_or_default();
                // FiveM escrow (Asset Escrow) files are encrypted and start with "FXAP".
                if bytes.starts_with(b"FXAP") {
                    return None;
                }
                let text = String::from_utf8_lossy(&bytes);
                let parse = cfx_lua_syntax::parse(&text);
                let lossless = parse.syntax().text() == text.as_ref();
                let errors = parse
                    .errors()
                    .iter()
                    .map(|e| {
                        let offset = u32::from(e.range.start()) as usize;
                        let (line, col) = line_col(&text, offset);
                        (line, col, e.message.clone())
                    })
                    .collect();
                Some(FileResult { path: path.clone(), bytes: bytes.len(), errors, lossless })
            })
            .collect()
    });
    let parse_time = parse_start.elapsed();

    let total_bytes: usize = results.iter().map(|r| r.bytes).sum();
    let mut error_files = 0;
    let mut error_count = 0;
    let mut not_lossless = 0;
    for FileResult { path, errors, lossless, .. } in &results {
        if !lossless {
            not_lossless += 1;
            println!("NOT LOSSLESS: {}", path.display());
        }
        if !errors.is_empty() {
            error_files += 1;
            error_count += errors.len();
            for (line, col, msg) in errors.iter().take(5) {
                println!("{}:{}:{}: {}", path.display(), line, col, msg);
            }
            if errors.len() > 5 {
                println!("{}: ... {} more", path.display(), errors.len() - 5);
            }
        }
    }
    println!(
        "\n{} escrow files skipped\n{} files, {:.1} MB, walk {:.0} ms, parse {:.0} ms ({} threads)\n{} files with errors, {} errors, {} not lossless",
        files.len() - results.len(),
        results.len(),
        total_bytes as f64 / 1048576.0,
        walk_time.as_secs_f64() * 1000.0,
        parse_time.as_secs_f64() * 1000.0,
        pool.current_num_threads(),
        error_files,
        error_count,
        not_lossless
    );
    if error_count > 0 || not_lossless > 0 {
        std::process::exit(1);
    }
}

fn line_col(text: &str, offset: usize) -> (usize, usize) {
    let before = &text.as_bytes()[..offset.min(text.len())];
    let line = before.iter().filter(|&&c| c == b'\n').count() + 1;
    let col = before.iter().rev().take_while(|&&c| c != b'\n').count() + 1;
    (line, col)
}
