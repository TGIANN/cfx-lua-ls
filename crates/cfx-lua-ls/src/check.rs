//! `cfx-lua-ls check <folder-or-file>...`: every diagnostic the server would show,
//! for the terminal (`text`), for tools and AI agents (`json`) or for code
//! scanning (`sarif`). Exit code 1 when a diagnostic reaches `--fail-on`.

use cfx_lua_ide::lint::{Lint, RULES, Severity};
use cfx_lua_ide::workspace::{Workspace, lint_file};
use cfx_lua_ide::{TextRange, TextSize};
use line_index::LineIndex;
use rayon::prelude::*;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// `[line, column, end line, end column]`, 1-based, columns in characters.
pub(crate) type Span = [u32; 4];

/// A fix's title and edits.
pub(crate) type FoundFix = (String, Vec<(Span, String)>);

/// One diagnostic with 1-based lines and columns (columns count characters).
#[derive(Debug, Clone)]
pub(crate) struct Found {
    pub(crate) file: PathBuf,
    pub(crate) range: Span,
    pub(crate) severity: Severity,
    pub(crate) code: String,
    pub(crate) message: String,
    pub(crate) fix: Option<FoundFix>,
}

pub(crate) fn position(index: &LineIndex, text: &str, offset: TextSize) -> (u32, u32) {
    let lc = index.line_col(offset.min(index.len()));
    let line_start = index.line(lc.line).map_or(0, |r| usize::from(r.start()));
    let col = text[line_start..line_start + lc.col as usize].chars().count() as u32;
    (lc.line + 1, col + 1)
}

pub(crate) fn span(index: &LineIndex, text: &str, r: TextRange) -> Span {
    let (a, b) = position(index, text, r.start());
    let (c, d) = position(index, text, r.end());
    [a, b, c, d]
}

/// Syntax errors and lint results for one file's text.
pub(crate) fn diagnose(ws: &Workspace, path: &Path, text: &str, config: &crate::config::Diagnostics) -> Vec<Found> {
    if cfx_lua_ide::is_escrow(text) {
        return Vec::new();
    }
    let parse = cfx_lua_ide::parse(text);
    let index = LineIndex::new(text);
    let mut out: Vec<Found> = cfx_lua_ide::syntax_diagnostics(&parse)
        .into_iter()
        .map(|d| Found {
            file: path.to_path_buf(),
            range: span(&index, text, d.range),
            severity: Severity::Error,
            code: d.code,
            message: d.message,
            fix: None,
        })
        .collect();
    if config.enable {
        let names = cfx_lua_ide::names(&parse, text);
        let lints = lint_file(ws, path, &parse, text, &names, &config.lint);
        out.extend(lints.iter().map(|l| found(path, &index, text, l)));
    }
    out
}

pub(crate) fn found(path: &Path, index: &LineIndex, text: &str, l: &Lint) -> Found {
    Found {
        file: path.to_path_buf(),
        range: span(index, text, l.range),
        severity: l.severity,
        code: l.code.to_string(),
        message: l.message.clone(),
        fix: l.fix.as_ref().map(|f| {
            (f.title.clone(), f.edits.iter().map(|(r, t)| (span(index, text, *r), t.clone())).collect())
        }),
    }
}

pub(crate) fn severity_name(s: Severity) -> &'static str {
    match s {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Information => "information",
        Severity::Hint => "hint",
    }
}

pub(crate) fn to_json(f: &Found, base: &Path) -> Value {
    let [line, column, end_line, end_column] = f.range;
    let mut v = json!({
        "file": relative(&f.file, base),
        "line": line, "column": column, "endLine": end_line, "endColumn": end_column,
        "severity": severity_name(f.severity),
        "code": f.code,
        "message": f.message,
    });
    if let Some((title, edits)) = &f.fix {
        let edits: Vec<Value> = edits
            .iter()
            .map(|([a, b, c, d], t)| json!({ "line": a, "column": b, "endLine": c, "endColumn": d, "newText": t }))
            .collect();
        v["fix"] = json!({ "title": title, "edits": edits });
    }
    v
}

fn relative(path: &Path, base: &Path) -> String {
    // `./a.lua` (a scan of `.`) is `a.lua`: code scanning resolves it against the repository.
    let p: PathBuf = path.strip_prefix(base).unwrap_or(path).components().filter(|c| !matches!(c, std::path::Component::CurDir)).collect();
    p.to_string_lossy().replace('\\', "/")
}

/// The files a set of paths covers, with the workspace that gives them context.
pub(crate) fn load(paths: &[PathBuf]) -> (Workspace, Vec<PathBuf>) {
    let folders: Vec<PathBuf> = paths.iter().filter(|p| p.is_dir()).cloned().collect();
    let mut ws = Workspace::scan(&folders);
    let mut files = ws.lua_files();
    for p in paths.iter().filter(|p| p.is_file()) {
        ws.ensure(p);
        if let Ok(text) = std::fs::read_to_string(p) {
            ws.update(p, &text);
        }
        files.push(p.clone());
    }
    files.sort();
    files.dedup();
    (ws, files)
}

/// The text with the quick fixes of the lints at `min` or above applied (those
/// that do not overlap); `None` when nothing changes. Files with syntax errors and
/// escrow files are never touched.
pub(crate) fn fix_text(
    ws: &Workspace,
    path: &Path,
    text: &str,
    config: &crate::config::Diagnostics,
    min: Severity,
) -> Option<(String, usize)> {
    if !config.enable || cfx_lua_ide::is_escrow(text) {
        return None;
    }
    let parse = cfx_lua_ide::parse(text);
    if !parse.errors().is_empty() {
        return None;
    }
    let names = cfx_lua_ide::names(&parse, text);
    let lints: Vec<Lint> =
        lint_file(ws, path, &parse, text, &names, &config.lint).into_iter().filter(|l| l.severity <= min).collect();
    let (out, n) = cfx_lua_ide::lint::apply_fixes(text, &lints);
    (n > 0).then_some((out, n))
}

fn level(s: &str) -> Option<Severity> {
    Severity::parse(s)
}

pub(crate) fn check_cmd(args: &[String]) {
    let mut format = "text".to_string();
    let mut all_files = false;
    let mut fix = false;
    let mut min = Severity::Warning;
    let mut fail_on = Severity::Error;
    let mut paths = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |name: &str| -> String {
            it.next().cloned().unwrap_or_else(|| {
                eprintln!("cfx-lua-ls: {name} needs a value");
                std::process::exit(2);
            })
        };
        match a.as_str() {
            "--format" => format = value("--format"),
            "--level" => min = level(&value("--level")).unwrap_or_else(|| usage()),
            "--fail-on" => fail_on = level(&value("--fail-on")).unwrap_or_else(|| usage()),
            "--all-files" => all_files = true,
            "--fix" => fix = true,
            s if s.starts_with("--") => usage(),
            _ => paths.push(PathBuf::from(a)),
        }
    }
    if paths.is_empty() {
        usage();
    }
    let started = std::time::Instant::now();
    let (ws, mut files) = load(&paths);
    // Files no manifest loads (notes, examples, templates saved as .lua) are left
    // out unless asked for or named on the command line.
    let before = files.len();
    if !all_files {
        files.retain(|f| paths.contains(f) || ws.info(f).loaded);
    }
    let skipped = before - files.len();
    let folders: Vec<PathBuf> = paths.iter().filter(|p| p.is_dir()).cloned().collect();
    let config = crate::config::load(&folders, None);
    if fix {
        // Only UTF-8 files: a lossy read written back would change the bytes.
        let done: Vec<usize> = files
            .par_iter()
            .filter_map(|f| {
                let text = String::from_utf8(std::fs::read(f).ok()?).ok()?;
                let (out, n) = fix_text(&ws, f, &text, &config, min)?;
                std::fs::write(f, out).ok()?;
                Some(n)
            })
            .collect();
        eprintln!("{} fixes applied in {} files", done.iter().sum::<usize>(), done.len());
    }
    let mut found: Vec<Found> = files
        .par_iter()
        .flat_map_iter(|f| {
            let text = std::fs::read(f).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
            diagnose(&ws, f, &text, &config)
        })
        .filter(|f| f.severity <= min)
        .collect();
    found.sort_by(|a, b| (&a.file, a.range).cmp(&(&b.file, b.range)));
    let base = std::env::current_dir().unwrap_or_default();
    match format.as_str() {
        "json" => {
            let mut summary = json!({ "files": files.len(), "skipped": skipped });
            for s in [Severity::Error, Severity::Warning, Severity::Information, Severity::Hint] {
                summary[severity_name(s)] = json!(found.iter().filter(|f| f.severity == s).count());
            }
            let list: Vec<Value> = found.iter().map(|f| to_json(f, &base)).collect();
            println!("{}", json!({ "diagnostics": list, "summary": summary }));
        }
        "sarif" => println!("{}", sarif(&found, &base)),
        "text" => {
            for f in &found {
                let [l, c, ..] = f.range;
                println!("{}:{l}:{c}: {} [{}] {}", relative(&f.file, &base), severity_name(f.severity), f.code, f.message);
            }
            let mut counts: Vec<(&str, usize)> = Vec::new();
            for f in &found {
                match counts.iter_mut().find(|(c, _)| *c == f.code) {
                    Some((_, n)) => *n += 1,
                    None => counts.push((&f.code, 1)),
                }
            }
            counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
            eprintln!(
                "{} files, {} resources, {} diagnostics in {:.2?}",
                files.len(),
                ws.resource_count(),
                found.len(),
                started.elapsed()
            );
            if skipped > 0 {
                eprintln!("{skipped} files no manifest loads were skipped (--all-files checks them)");
            }
            for (code, n) in counts {
                eprintln!("  {n:>6}  {code}");
            }
        }
        _ => usage(),
    }
    if found.iter().any(|f| f.severity <= fail_on) {
        std::process::exit(1);
    }
}

fn sarif(found: &[Found], base: &Path) -> Value {
    let level = |s: Severity| match s {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Information | Severity::Hint => "note",
    };
    let rules: Vec<Value> = RULES
        .iter()
        .map(|(id, s)| {
            json!({
                "id": id,
                "shortDescription": { "text": cfx_lua_ide::lint::describe(id).unwrap_or_default() },
                "defaultConfiguration": { "level": s.map_or("none", level) }
            })
        })
        .collect();
    let results: Vec<Value> = found
        .iter()
        .map(|f| {
            let [a, b, c, d] = f.range;
            json!({
                "ruleId": f.code,
                "level": level(f.severity),
                "message": { "text": f.message },
                "locations": [{ "physicalLocation": {
                    "artifactLocation": { "uri": relative(&f.file, base), "uriBaseId": "%SRCROOT%" },
                    "region": { "startLine": a, "startColumn": b, "endLine": c, "endColumn": d }
                } }]
            })
        })
        .collect();
    json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": { "driver": {
                "name": "cfx-lua-ls",
                "version": env!("CARGO_PKG_VERSION"),
                "informationUri": "https://github.com/TGIANN/cfx-lua-ls",
                "rules": rules
            } },
            "columnKind": "unicodeCodePoints",
            "results": results
        }]
    })
}

fn usage() -> ! {
    eprintln!(
        "usage: cfx-lua-ls check <folder-or-file>... [--format text|json|sarif] [--level error|warning|information|hint] [--fail-on error|warning|information|hint] [--all-files] [--fix]"
    );
    std::process::exit(2);
}
