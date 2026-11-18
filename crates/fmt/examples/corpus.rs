//! Formatter checks over folders of Lua files.
//!
//!   corpus list <list.txt> <dir>...       every `.lua` file (escrow files left out)
//!   corpus check <list.txt> <luals_out> [<luals_out2>]
//!
//! `check` formats each file with the default style and compares with the output of
//! `tools/luals_format.lua` (the first differences saved as
//! `<luals_out>/diff/<n>.ours.lua`). `<luals_out2>` is LuaLS run again on its own
//! output: LuaLS is not idempotent (a lazy break inside a one-line block opens the
//! whole block on the next run), so a second run of ours is compared with it. It
//! also reports files that change on a second run, output that does not reparse,
//! and changed tokens or comments.
//!
//!   corpus check-range <list.txt> <luals_range_out>
//!
//! Range formatting of a few line ranges per file against
//! `tools/luals_range_format.lua`: same text and same replaced lines.

use std::fs;
use std::path::{Path, PathBuf};

use cfx_lua_fmt::{Style, format};
use cfx_lua_syntax::SyntaxKind;

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let path = e.path();
        if path.is_dir() {
            walk(&path, out);
        } else if path.extension().is_some_and(|x| x == "lua") {
            out.push(path);
        }
    }
}

/// Non-whitespace tokens; multi-line text compared line by line without the
/// trailing spaces the formatter drops. A `;` counts as `,`: the default
/// `table_separator_style = comma` turns table separators into commas.
fn tokens(text: &str) -> Vec<(SyntaxKind, String)> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let (tokens, _) = cfx_lua_syntax::lexer::tokenize(text);
    let mut pos = 0;
    let mut out = Vec::new();
    for t in tokens {
        let len = u32::from(t.len) as usize;
        let s = &text[pos..pos + len];
        pos += len;
        if t.kind == SyntaxKind::WHITESPACE {
            continue;
        }
        if t.kind == SyntaxKind::SEMICOLON {
            out.push((SyntaxKind::COMMA, ",".to_string()));
            continue;
        }
        let s = s.replace("\r\n", "\n").replace('\r', "\n");
        let s: Vec<&str> = s.split('\n').map(|l| l.trim_end_matches([' ', '\t'])).collect();
        out.push((t.kind, s.join("\n")));
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("list") => {
            let mut files = Vec::new();
            for dir in &args[2..] {
                walk(Path::new(dir), &mut files);
            }
            files.retain(|p| fs::read(p).is_ok_and(|b| !b.starts_with(b"FXAP")));
            let list: Vec<String> = files.iter().map(|p| p.display().to_string()).collect();
            fs::write(&args[1], list.join("\n") + "\n").unwrap();
            println!("{} files", list.len());
        }
        Some("check") => check(&args[1], Path::new(&args[2]), args.get(3).map(Path::new)),
        Some("check-range") => check_range(&args[1], Path::new(&args[2])),
        _ => eprintln!(
            "usage: corpus list <list.txt> <dir>... | corpus check <list.txt> <luals_out> [<luals_out2>] | corpus check-range <list.txt> <luals_range_out>"
        ),
    }
}

/// The ranges `tools/luals_range_format.lua` asks for.
fn ranges(text: &str) -> Vec<(usize, usize)> {
    let lines = text.matches('\n').count() + 1;
    [0.1, 0.3, 0.5, 0.7, 0.9].iter().map(|f| ((lines as f64 * f).floor() as usize, 0)).map(|(s, _)| (s, s + 3)).collect()
}

/// `(start, end, Some((text, replaced_start, replaced_end)))` per range.
type RangeResult = (usize, usize, Option<(String, usize, usize)>);

fn parse_luals_ranges(data: &[u8]) -> Vec<RangeResult> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < data.len() {
        let eol = pos + data[pos..].iter().position(|&b| b == b'\n').unwrap();
        let header = std::str::from_utf8(&data[pos..eol]).unwrap();
        let v: Vec<usize> = header.split(' ').map(|x| x.parse().unwrap()).collect();
        pos = eol + 1;
        let text = String::from_utf8_lossy(&data[pos..pos + v[5]]).into_owned();
        pos += v[5];
        out.push((v[0], v[1], (v[2] == 1).then_some((text, v[3], v[4]))));
    }
    out
}

fn check_range(list: &str, luals: &Path) {
    let style = Style::default();
    let (mut compared, mut equal) = (0, 0);
    let mut shown = 0;
    for (i, path) in fs::read_to_string(list).unwrap().lines().enumerate() {
        let Ok(text) = fs::read_to_string(path) else { continue };
        let Ok(data) = fs::read(luals.join(format!("{}.txt", i + 1))) else { continue };
        if cfx_lua_fmt::format(&text, &style).is_none() {
            continue;
        }
        let theirs = parse_luals_ranges(&data);
        for ((start, end), (ts, te, t)) in ranges(&text).into_iter().zip(theirs) {
            assert_eq!((start, end), (ts, te), "{path}: range list differs");
            let ours = cfx_lua_fmt::format_range(&text, &style, start, end).map(|e| (e.text, e.start_line, e.end_line));
            compared += 1;
            if ours == t {
                equal += 1;
            } else if shown < 10 {
                shown += 1;
                println!("--- {path} lines {start}..={end}\n### LuaLS {t:?}\n### ours {ours:?}");
            }
        }
    }
    println!("\nranges compared with LuaLS: {compared}, equal {equal}, different {}", compared - equal);
}

fn check(list: &str, luals: &Path, luals2: Option<&Path>) {
    let style = Style::default();
    let diff_dir = luals.join("diff");
    let _ = fs::create_dir_all(&diff_dir);
    let (mut files, mut unreadable, mut ours_refused, mut tree_refused, mut luals_refused) = (0, 0, 0, 0, 0);
    let (mut compared, mut equal, mut saved) = (0, 0, 0);
    let (mut compared2, mut equal2) = (0, 0);
    let mut not_idempotent = Vec::new();
    let mut reparse_errors = Vec::new();
    let mut token_changes = Vec::new();
    let start = std::time::Instant::now();
    for (i, path) in fs::read_to_string(list).unwrap().lines().enumerate() {
        let n = i + 1;
        files += 1;
        let Ok(text) = fs::read_to_string(path) else {
            unreadable += 1;
            continue;
        };
        let Some(ours) = format(&text, &style) else {
            if cfx_lua_syntax::parse(text.strip_prefix('\u{feff}').unwrap_or(&text)).errors().is_empty() {
                // Our parser accepts it but the port of their grammar does not.
                tree_refused += 1;
                println!("tree refused: {path}");
            } else {
                ours_refused += 1;
            }
            continue;
        };
        let second = format(&ours, &style);
        if second.as_deref() != Some(ours.as_str()) {
            not_idempotent.push(path.to_string());
        }
        if let Some(Ok(theirs)) = luals2.map(|d| fs::read_to_string(d.join(format!("{n}.lua")))) {
            compared2 += 1;
            if second.as_deref() == Some(theirs.as_str()) {
                equal2 += 1;
            } else {
                println!("second run differs: {n} {path}");
            }
        }
        if !cfx_lua_syntax::parse(ours.strip_prefix('\u{feff}').unwrap_or(&ours)).errors().is_empty() {
            reparse_errors.push(path.to_string());
        }
        let (before, after) = (tokens(&text), tokens(&ours));
        if before != after {
            let i = before.iter().zip(&after).position(|(a, b)| a != b).unwrap_or(before.len().min(after.len()));
            token_changes.push(format!("{path}: {:?} became {:?}", before.get(i), after.get(i)));
        }
        match fs::read_to_string(luals.join(format!("{n}.lua"))) {
            Ok(theirs) => {
                compared += 1;
                if theirs == ours {
                    equal += 1;
                } else if saved < 30 {
                    saved += 1;
                    fs::write(diff_dir.join(format!("{n}.ours.lua")), &ours).unwrap();
                    println!("differs: {n} {path}");
                }
            }
            Err(_) => luals_refused += 1,
        }
    }
    let show = |name: &str, v: &[String]| {
        println!("{name}: {}", v.len());
        for p in v.iter().take(10) {
            println!("  {p}");
        }
    };
    println!(
        "\n{files} files ({unreadable} unreadable), {:.1} s\nrefused: {ours_refused} by our parser, {tree_refused} by their grammar port, {luals_refused} more by LuaLS\ncompared with LuaLS: {compared}, equal {equal}, different {}",
        start.elapsed().as_secs_f64(),
        compared - equal
    );
    if luals2.is_some() {
        println!("second run compared with LuaLS: {compared2}, equal {equal2}, different {}", compared2 - equal2);
    }
    show("not idempotent", &not_idempotent);
    show("output with syntax errors", &reparse_errors);
    show("tokens or comments changed", &token_changes);
}
