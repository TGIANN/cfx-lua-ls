//! Corpus gate for the editor features: `cargo run --release -p cfx-lua-ide --example gate -- <folder>...`.
//! - Document symbols: every symbol lies inside its file, holds its selection range
//!   and sits inside its parent's value. (Anonymous functions and tables can have an
//!   empty name, as in LuaLS; the server shows it as a space.)
//! - Folding: every fold lies inside its file, in order of its start.
//! - Selection: at every 97th byte and at the end, each step strictly holds the one
//!   before and the last is the whole file.
//! - Semantic tokens: none empty, none overlapping, in order, inside the file.
//! - Names: every reference is a name token of its own text, references in order,
//!   every declaration a name (or `:` for `self`), each reference's target a
//!   declaration of the same name declared before it (labels: anywhere).
//!
//! - Field references: at every 20th `.` / `:` name, every site found gives
//!   the same set back (symmetric); the slowest call is printed.
//! - Hover, completion, signature help: at every 997th character, no panic; a
//!   hover range holds the offset, completion labels are not empty.
//!
//! Prints the failures, the counts and the time each feature took (parsing excluded).

use cfx_lua_ide::{
    DeclKind, DocumentSymbol, Target, TextRange, TextSize, complete, document_symbols, field_references, folding_ranges, hover, is_escrow,
    names, parse, selection_ranges, semantic_tokens, signature_help, FileCtx,
};
use cfx_lua_ide::library::Game;
use cfx_lua_ide::manifest::FileSide;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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

fn check(
    path: &Path,
    len: TextSize,
    parent: Option<TextRange>,
    syms: &[DocumentSymbol],
    bad: &mut Vec<String>,
    count: &mut usize,
) {
    for s in syms {
        *count += 1;
        let mut why = Vec::new();
        if s.range.end() > len {
            why.push("range past the end");
        }
        if !s.range.contains_range(s.selection_range) {
            why.push("selection outside range");
        }
        if parent.is_some_and(|p| !p.contains_range(s.selection_range)) {
            why.push("selection outside parent value");
        }
        if !why.is_empty() {
            bad.push(format!(
                "{}: {:?} {:?} {:?}: {}",
                path.display(),
                s.name,
                s.range,
                s.selection_range,
                why.join(", ")
            ));
        }
        check(path, len, s.value_range, &s.children, bad, count);
    }
}

fn main() {
    let mut files = Vec::new();
    for arg in std::env::args().skip(1) {
        collect(Path::new(&arg), &mut files);
    }
    let mut bad = Vec::new();
    let (mut checked, mut count, mut spent) = (0, 0, Duration::ZERO);
    let (mut folds, mut fold_spent) = (0, Duration::ZERO);
    let (mut selections, mut sel_spent) = (0, Duration::ZERO);
    let (mut highlights, mut hl_spent) = (0, Duration::ZERO);
    let (mut refs, mut names_spent) = (0, Duration::ZERO);
    let (mut assists, mut assist_spent) = (0, Duration::ZERO);
    let (mut field_calls, mut field_spent, mut field_slowest) = (0usize, Duration::ZERO, (Duration::ZERO, String::new()));
    // Workspace-backed features (framework objects, `lib`, exports) at every
    // 20th `.` / `:`: the slowest call is reported.
    let roots: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    let ws = cfx_lua_ide::workspace::Workspace::scan(&roots);
    let (mut ws_calls, mut ws_spent, mut ws_slowest, mut ws_hits) = (0usize, Duration::ZERO, (Duration::ZERO, String::new()), 0usize);
    let (mut inlay_count, mut inlay_spent, mut inlay_slowest) = (0usize, Duration::ZERO, (Duration::ZERO, String::new()));
    for path in &files {
        let Ok(text) = std::fs::read_to_string(path) else { continue };
        if is_escrow(&text) {
            continue;
        }
        checked += 1;
        let p = parse(&text);
        let t = Instant::now();
        let syms = document_symbols(&p, &text);
        spent += t.elapsed();
        check(path, TextSize::of(&text), None, &syms, &mut bad, &mut count);

        let t = Instant::now();
        let ranges = folding_ranges(&p, &text);
        fold_spent += t.elapsed();
        folds += ranges.len();
        if let Some(f) = ranges.iter().find(|f| f.range.end() > TextSize::of(&text)) {
            bad.push(format!("{}: fold {:?} past the end", path.display(), f.range));
        }
        if ranges.windows(2).any(|w| w[0].range.start() > w[1].range.start()) {
            bad.push(format!("{}: folds out of order", path.display()));
        }

        let len = TextSize::of(&text);
        for at in (0..u32::from(len)).step_by(97).map(TextSize::from).chain([len]) {
            let t = Instant::now();
            let steps = selection_ranges(&p, at);
            sel_spent += t.elapsed();
            selections += 1;
            let nested = steps.windows(2).all(|w| w[1].contains_range(w[0]) && w[1] != w[0]);
            let whole = steps.last() == Some(&TextRange::up_to(len)) || (len == 0.into() && steps.is_empty());
            if !nested || !whole {
                bad.push(format!("{}: selection at {at:?}: {steps:?}", path.display()));
            }
        }

        let t = Instant::now();
        let toks = semantic_tokens(&p);
        hl_spent += t.elapsed();
        highlights += toks.len();
        let ordered = toks.windows(2).all(|w| w[0].range.end() <= w[1].range.start());
        if !ordered || toks.iter().any(|t| t.range.is_empty() || t.range.end() > len) {
            bad.push(format!("{}: semantic tokens empty, overlapping or past the end", path.display()));
        }

        let t = Instant::now();
        let n = names(&p, &text);
        names_spent += t.elapsed();
        refs += n.refs.len();
        let is_name = |r: TextRange| {
            let s = &text[r];
            !s.is_empty() && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80)
        };
        if n.refs.windows(2).any(|w| w[0].range.end() > w[1].range.start()) {
            bad.push(format!("{}: name references out of order", path.display()));
        }
        for r in &n.refs {
            let ok = is_name(r.range)
                && match r.target {
                    Target::Global => true,
                    Target::Decl(d) => {
                        let decl = &n.decls[d];
                        decl.name == text[r.range]
                            && (decl.kind == DeclKind::Label || decl.range.start() <= r.range.start())
                    }
                };
            if !ok {
                bad.push(format!("{}: reference {:?} {:?}", path.display(), r.range, &text[r.range]));
                break;
            }
        }
        if let Some(d) = n.decls.iter().find(|d| d.kind != DeclKind::SelfParam && (!is_name(d.range) || text[d.range] != d.name)) {
            bad.push(format!("{}: declaration {:?}", path.display(), d.range));
        }

        let dots: Vec<TextSize> = text
            .match_indices(['.', ':'])
            .filter(|(i, _)| text[i + 1..].starts_with(|c: char| c.is_ascii_alphabetic() || c == '_'))
            .map(|(i, _)| TextSize::from(i as u32 + 1))
            .step_by(20)
            .collect();
        for at in dots {
            let t = Instant::now();
            let found = field_references(&p, &n, at);
            let e = t.elapsed();
            field_spent += e;
            field_calls += 1;
            if e > field_slowest.0 {
                field_slowest = (e, format!("{}:{}", path.display(), u32::from(at)));
            }
            let Some(sites) = found else { continue };
            let mut set: Vec<TextRange> = sites.iter().map(|s| s.0).collect();
            set.sort_by_key(|r| (r.start(), r.end()));
            for (r, _) in sites.iter().take(3) {
                let mut back: Vec<TextRange> = field_references(&p, &n, r.start()).unwrap_or_default().into_iter().map(|s| s.0).collect();
                back.sort_by_key(|r| (r.start(), r.end()));
                if back != set {
                    bad.push(format!("{}: field references at {at:?} and {:?} differ", path.display(), r.start()));
                    break;
                }
            }
        }

        for (at, _) in text.char_indices().step_by(997) {
            let at = TextSize::from(at as u32);
            let t = Instant::now();
            let h = hover(&p, &n, &text, at, FileCtx::plain(Game::Gta5, FileSide::Unknown));
            let c = complete(&p, &n, &text, at, FileCtx::plain(Game::Gta5, FileSide::Client));
            let s = signature_help(&p, &n, &text, at, FileCtx::plain(Game::Gta5, FileSide::Unknown));
            assist_spent += t.elapsed();
            assists += 1;
            if h.is_some_and(|h| !h.range.contains_inclusive(at))
                || c.is_some_and(|c| c.items.iter().any(|i| i.label.is_empty()))
                || s.is_some_and(|s| s.signatures.is_empty())
            {
                bad.push(format!("{}: hover / completion / signature at {at:?}", path.display()));
            }
        }
        let info = ws.info(path);
        let cx = FileCtx { game: info.game, side: info.side, ws: Some((&ws, path.as_path())) };
        for (i, _) in text.match_indices(['.', ':']).step_by(20) {
            let at = TextSize::from(i as u32 + 1);
            // `GATE_TRACE=1`: the position about to be asked, for a crash.
            if std::env::var_os("GATE_TRACE").is_some() {
                eprintln!("{}:{i}", path.display());
            }
            let t = Instant::now();
            let c = complete(&p, &n, &text, at, cx);
            let h = hover(&p, &n, &text, at, cx);
            let s = signature_help(&p, &n, &text, at, cx);
            let _ = cfx_lua_ide::api_definition(&p, &n, &text, at, &ws, path);
            let e = t.elapsed();
            ws_calls += 1;
            ws_spent += e;
            if e > ws_slowest.0 {
                ws_slowest = (e, format!("{}:{i}", path.display()));
            }
            if h.as_ref().is_some_and(|h| h.markdown.contains("\n\n`")) {
                ws_hits += 1;
            }
            if h.is_some_and(|h| !h.range.contains_inclusive(at))
                || c.is_some_and(|c| c.items.iter().any(|i| i.label.is_empty()))
                || s.is_some_and(|s| s.signatures.is_empty())
            {
                bad.push(format!("{}: workspace hover / completion / signature at {at:?}", path.display()));
            }
        }
        // Inlay hints a screen (4 KB) at a time, as an editor asks.
        for start in (0..u32::from(len)).step_by(4096) {
            let window = TextRange::new(start.into(), (start + 4096).min(u32::from(len)).into());
            if std::env::var_os("GATE_TRACE").is_some() {
                eprintln!("inlay {}:{start}", path.display());
            }
            let t = Instant::now();
            let hints = cfx_lua_ide::inlay_hints(&p, &n, &text, window, cx);
            let e = t.elapsed();
            inlay_spent += e;
            inlay_count += hints.len();
            if e > inlay_slowest.0 {
                inlay_slowest = (e, format!("{}:{start}", path.display()));
            }
            if hints.windows(2).any(|w| w[0].offset > w[1].offset) || hints.iter().any(|h| h.offset > len || h.label.is_empty()) {
                bad.push(format!("{}: inlay hints at {start}", path.display()));
            }
        }
    }
    println!("workspace features: {ws_calls} positions in {ws_spent:?}, {ws_hits} hovers from other files, slowest {:?} at {}", ws_slowest.0, ws_slowest.1);
    println!("field references: {field_calls} positions in {field_spent:?}, slowest {:?} at {}", field_slowest.0, field_slowest.1);
    println!("inlay hints: {inlay_count} in {inlay_spent:?}, slowest 4 KB window {:?} at {}", inlay_slowest.0, inlay_slowest.1);
    for b in bad.iter().take(30) {
        println!("{b}");
    }
    println!(
        "{} files found, {checked} checked (readable, not escrow), {} bad
{count} symbols in {spent:?}, {folds} folds in {fold_spent:?}, {selections} selections in {sel_spent:?}, {highlights} semantic tokens in {hl_spent:?}, {refs} name references in {names_spent:?}, {assists} hover + completion + signature in {assist_spent:?}",
        files.len(),
        bad.len()
    );
}
