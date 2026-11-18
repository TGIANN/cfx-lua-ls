//! Parses every run of `---` comment lines under the given folders and reports
//! doc errors and descriptions that look like a type the parser stopped short
//! of, grouped by their first character.
//!
//!     cargo run --release -p cfx-lua-ide --example doc_probe -- <folder>...

use std::collections::BTreeMap;
use std::path::Path;

use cfx_lua_ide::TextSize;
use cfx_lua_ide::luacats::{Tag, parse_block};

fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name();
        let name = name.to_string_lossy();
        if p.is_dir() {
            if !matches!(name.as_ref(), "node_modules" | ".git" | "stream") {
                walk(&p, out);
            }
        } else if name.ends_with(".lua") {
            out.push(p);
        }
    }
}

fn main() {
    let mut files = Vec::new();
    for a in std::env::args().skip(1) {
        walk(Path::new(&a), &mut files);
    }
    let mut tags_seen = 0usize;
    let mut errors: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    let mut suspicious: BTreeMap<char, Vec<String>> = BTreeMap::new();
    let t = std::time::Instant::now();
    for f in &files {
        let Ok(text) = std::fs::read_to_string(f) else { continue };
        let mut run = String::new();
        let mut flush = |run: &mut String| {
            if run.is_empty() {
                return;
            }
            let (tags, errs) = parse_block(run, TextSize::from(0));
            tags_seen += tags.len();
            for e in errs {
                let at = u32::from(e.range.start()) as usize;
                let line_start = run[..at.min(run.len())].rfind('\n').map_or(0, |i| i + 1);
                let line = run[line_start..].lines().next().unwrap_or("");
                errors.entry(e.code).or_default().push(format!("{}: {line}", f.display()));
            }
            for tag in tags {
                let desc = match &tag.tag {
                    Tag::Field { desc, .. } | Tag::Param { desc, .. } | Tag::Return { desc, .. } => desc.as_str(),
                    _ => "",
                };
                if let Some(c) = desc.chars().next().filter(|c| "|<>[],()?:{}.".contains(*c)) {
                    let at = u32::from(tag.range.start()) as usize;
                    let line = run[at.min(run.len())..].lines().next().unwrap_or("");
                    suspicious.entry(c).or_default().push(format!("{}: {line}", f.display()));
                }
            }
            run.clear();
        };
        for line in text.lines() {
            if line.trim_start().starts_with("---") {
                run.push_str(line.trim_start());
                run.push('\n');
            } else {
                flush(&mut run);
            }
        }
        flush(&mut run);
    }
    println!("{} files, {tags_seen} tags, {:?}", files.len(), t.elapsed());
    for (code, v) in &errors {
        println!("{code}: {}", v.len());
        for s in v.iter().take(8) {
            println!("    {s}");
        }
    }
    for (c, v) in &suspicious {
        println!("description starting with {c:?}: {}", v.len());
        for s in v.iter().take(8) {
            println!("    {s}");
        }
    }
}
