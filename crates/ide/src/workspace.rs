//! The workspace: resources (folders with an `fxmanifest.lua` / `__resource.lua`),
//! their files, the side each file runs on and the globals each file defines.
//!
//! Every resource runs in Lua states of its own, one per side, so a global is
//! visible to the files of the same resource on the same side (shared files on
//! both) and through the manifest's `@resource/file.lua` imports. Closed files keep
//! only this summary, never a tree.

mod query;

pub use query::{ApiItem, Exports, FileRanges, TypeTarget, WsSymbol, WsSymbolKind};

use crate::api::FileApi;
use crate::library::Game;
use crate::lint::{Context, LintConfig};
use crate::manifest::{FileSide, Manifest};
use crate::names::{Access, Names, Target};
use crate::{Parse, is_escrow};
use cfx_lua_syntax::SyntaxKind::*;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Globals a file defines: `X = ...`, `function X()`, `_G.X = ...`, `_ENV.X = ...`,
/// `rawset(_G, 'X', ...)`.
pub fn global_writes(parse: &Parse, text: &str, names: &Names) -> Vec<String> {
    global_writes_in(&parse.syntax(), text, names)
}

pub(crate) fn global_writes_in(root: &cfx_lua_syntax::SyntaxNode, text: &str, names: &Names) -> Vec<String> {
    let mut out: Vec<String> = names
        .refs
        .iter()
        .filter(|r| r.target == Target::Global && r.access == Access::Write)
        .map(|r| text[r.range].to_string())
        .collect();
    // `_G`, `_ENV`, `_ENV._G`, `_G._G._G`.
    fn is_env(text: &str, names: &Names, n: &cfx_lua_syntax::SyntaxNode) -> bool {
        match n.kind() {
            NAME_EXPR => {
                matches!(&text[n.text_range()], "_G" | "_ENV")
                    && names.refs.iter().any(|r| r.range == n.text_range() && r.target == Target::Global)
            }
            INDEX_EXPR => {
                crate::lint::field_name(n).is_some_and(|f| f.text() == "_G") && n.first_child().is_some_and(|o| is_env(text, names, &o))
            }
            _ => false,
        }
    }
    let is_env = |n: &cfx_lua_syntax::SyntaxNode| is_env(text, names, n);
    for node in root.descendants() {
        match node.kind() {
            INDEX_EXPR if node.parent().is_some_and(|p| p.kind() == VAR_LIST) => {
                if node.first_child().is_some_and(|o| is_env(&o))
                    && let Some(field) = crate::lint::field_name(&node)
                {
                    out.push(field.text().to_string());
                }
            }
            CALL_EXPR => {
                let callee = node.first_child();
                if callee.as_ref().is_some_and(|c| &text[c.text_range()] == "rawset")
                    && let Some(args) = node.children().find(|c| c.kind() == ARG_LIST)
                {
                    let a: Vec<_> = args.children().collect();
                    if a.len() == 3
                        && is_env(&a[0])
                        && a[1].kind() == LITERAL_EXPR
                        && let Some(s) = crate::util::unquote(text[a[1].text_range()].trim())
                    {
                        out.push(s.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Globals a file reads or changes a field of, without defining them.
fn global_uses(text: &str, names: &Names) -> HashSet<String> {
    names
        .refs
        .iter()
        .filter(|r| r.target == Target::Global && r.access != Access::Write)
        .map(|r| text[r.range].to_string())
        .collect()
}

/// A path as a map key: `/` separators, lower case on Windows.
pub fn key(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    if cfg!(windows) { s.to_lowercase() } else { s }
}

#[derive(Debug, Default)]
struct FileEntry {
    path: PathBuf,
    resource: Option<usize>,
    /// Relative to the resource folder, `/` separators.
    rel: String,
    globals: HashSet<String>,
    uses: HashSet<String>,
    escrow: bool,
    api: FileApi,
}

#[derive(Debug)]
struct Resource {
    root: PathBuf,
    name: String,
    manifest: Manifest,
    escrow: bool,
    /// A file of the resource loads Lua code at run time.
    loader: bool,
    files: Vec<String>,
    /// Exports registered by the resource's JavaScript scripts, by side.
    js_exports: Vec<(String, FileSide)>,
    /// A script whose exports cannot be read (a computed name in JavaScript, a
    /// glob over JavaScript files, a .NET assembly, a Lua script missing on disk).
    foreign: bool,
}

/// `exports('name', ...)` in a JavaScript file: the literal names, and whether
/// some name is computed.
fn js_exports(text: &str) -> (Vec<String>, bool) {
    let (mut names, mut dynamic) = (Vec::new(), text.contains("__cfx_export_"));
    let b = text.as_bytes();
    // `t=exports` / `(exports)`: the function under another name (minified
    // bundles), so the names it registers are not literal `exports(` calls.
    let mut at = 0;
    while let Some(i) = text[at..].find("exports") {
        let s = at + i;
        at = s + "exports".len();
        let word_before = s > 0 && (b[s - 1].is_ascii_alphanumeric() || b[s - 1] == b'_' || b[s - 1] == b'$' || b[s - 1] == b'.');
        let next = text[at..].trim_start().chars().next();
        let word_after = text[at..].chars().next().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
        if !word_before && !word_after && matches!(next, Some(',' | ')' | ';' | '}' | ']') | None) {
            dynamic = true;
        }
    }
    let mut at = 0;
    while let Some(i) = text[at..].find("exports(") {
        let s = at + i;
        at = s + "exports(".len();
        // `exports(` itself, `global.exports(`, `globalThis.exports(`; not `module.exports(`.
        let before = &text[..s];
        let prefixed = before.ends_with("global.") || before.ends_with("globalThis.");
        if !prefixed && s > 0 && (b[s - 1].is_ascii_alphanumeric() || b[s - 1] == b'_' || b[s - 1] == b'.' || b[s - 1] == b'$') {
            continue;
        }
        let rest = text[at..].trim_start();
        match rest.chars().next() {
            Some(q @ ('\'' | '"')) => match rest[1..].find(q) {
                Some(end) => names.push(rest[1..1 + end].to_string()),
                None => dynamic = true,
            },
            _ => dynamic = true,
        }
    }
    (names, dynamic)
}

/// A regular file's bytes, `None` above `max` (checked on the open handle and
/// while reading, so a file growing in between cannot pass).
fn read_capped(path: &Path, max: u64) -> Option<Vec<u8>> {
    use std::io::Read;
    let f = std::fs::File::open(path).ok()?;
    let m = f.metadata().ok()?;
    if !m.is_file() || m.len() > max {
        return None;
    }
    let mut bytes = Vec::with_capacity(m.len() as usize);
    f.take(max + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= max).then_some(bytes)
}

/// The words of a doc comment line that may name a type (`Opts`, `Nui.Faq`).
fn doc_words(line: &str, out: &mut Vec<String>) {
    let mut word = String::new();
    for ch in line.chars().chain(std::iter::once(' ')) {
        if ch.is_alphanumeric() || ch == '_' || (ch == '.' && !word.is_empty()) {
            word.push(ch);
        } else if !word.is_empty() {
            let w = std::mem::take(&mut word);
            let w = w.trim_end_matches('.');
            if w.starts_with(|c: char| c.is_alphabetic() || c == '_') {
                out.push(w.to_string());
            }
        }
    }
}

/// The names a doc type mentions.
pub(super) fn type_names(t: &crate::luacats::DocType, out: &mut Vec<String>) {
    use crate::luacats::DocType as D;
    match t {
        D::Name(n) => out.push(n.clone()),
        D::Generic(n, args) => {
            out.push(n.clone());
            args.iter().for_each(|a| type_names(a, out));
        }
        D::Array(i) | D::Optional(i) => type_names(i, out),
        D::Union(ts) | D::Tuple(ts) => ts.iter().for_each(|a| type_names(a, out)),
        D::Fun(f) => {
            f.params.iter().filter_map(|p| p.ty.as_ref()).for_each(|a| type_names(a, out));
            f.rets.iter().for_each(|r| type_names(&r.ty, out));
        }
        D::Table(fields) => fields.iter().for_each(|f| type_names(&f.ty, out)),
        D::Str(..) | D::Int(_) | D::Bool(_) | D::Code(_) | D::Dots => {}
    }
}

/// The exports of a resource's non-Lua scripts.
fn foreign_scripts(root: &Path, manifest: &Manifest) -> (Vec<(String, FileSide)>, bool) {
    let mut out = Vec::new();
    let mut foreign = false;
    for (list, side) in [(&manifest.client, FileSide::Client), (&manifest.server, FileSide::Server), (&manifest.shared, FileSide::Shared)] {
        // A Lua script the manifest names but the folder lacks: its exports are unknown.
        for entry in list.iter().filter(|e| !e.starts_with('@') && !e.contains('*') && e.to_ascii_lowercase().ends_with(".lua")) {
            let inside = !entry.contains('\\')
                && Path::new(entry.as_str()).components().all(|c| matches!(c, std::path::Component::Normal(_) | std::path::Component::CurDir));
            if !inside || !root.join(entry).is_file() {
                foreign = true;
            }
        }
        for entry in list.iter().filter(|e| !e.starts_with('@') && !e.to_ascii_lowercase().ends_with(".lua")) {
            // Only plain relative paths inside the resource: a manifest from an
            // opened folder must not make the server read `C:\...` or `\\host\share`.
            let inside = !entry.contains('\\')
                && Path::new(entry.as_str()).components().all(|c| matches!(c, std::path::Component::Normal(_) | std::path::Component::CurDir));
            if !entry.to_ascii_lowercase().ends_with(".js") || entry.contains('*') || !inside {
                foreign = true;
                continue;
            }
            match read_capped(&root.join(entry), 32 << 20) {
                Some(bytes) => {
                    let (names, dynamic) = js_exports(&String::from_utf8_lossy(&bytes));
                    foreign |= dynamic;
                    out.extend(names.into_iter().map(|n| (n, side)));
                }
                None => foreign = true,
            }
        }
    }
    (out, foreign)
}

#[derive(Debug, Default)]
pub struct Workspace {
    folders: Vec<PathBuf>,
    /// LuaLS's `Lua.workspace.library`: read for definitions only.
    library: Vec<PathBuf>,
    resources: Vec<Resource>,
    /// Resource by folder key and by lower-case name.
    by_root: HashMap<String, usize>,
    by_name: HashMap<String, usize>,
    files: HashMap<String, FileEntry>,
    /// Names some file documents as `---@deprecated`, with the number of files.
    deprecated: HashMap<String, usize>,
    /// `__cfx_export_...` literals, with the number of files.
    export_events: HashMap<String, usize>,
    /// Resource names found in more than one folder: which copy runs is unknown.
    duplicate_names: HashSet<String>,
    /// Event names, with the number of files that send them to the server, fire
    /// them with `TriggerEvent`, or name them in another string literal.
    server_bound: HashMap<String, usize>,
    fired_locally: HashMap<String, usize>,
    event_strings: HashMap<String, usize>,
    /// Class name to the files that declare it or bind a value to it.
    classes: HashMap<String, Vec<String>>,
    /// `---@alias` / `---@enum` names, with the number of files.
    doc_names: HashMap<String, usize>,
}

/// What lint needs to know about one file.
#[derive(Debug, Clone)]
pub struct FileInfo {
    pub side: FileSide,
    pub game: Game,
    pub resource: Option<String>,
    pub ox_lib: bool,
    pub globals_unknown: bool,
    pub manifest: bool,
    /// The resource's manifest loads the file (always true outside resources).
    pub loaded: bool,
}

/// Globals these files are known to define, added to what the workspace's copy
/// shows (and used alone when the resource is not in the workspace). ox_lib's
/// `locale` comes from a module `init.lua` loads when the manifest asks for it.
const KNOWN_IMPORTS: &[(&str, &[&str])] = &[
    ("@ox_lib/init.lua", &["lib", "cache", "locale", "SetInterval", "ClearInterval", "noop", "require"]),
    ("@oxmysql/lib/mysql.lua", &["MySQL"]),
    ("@es_extended/imports.lua", &["ESX"]),
    ("@es_extended/locale.lua", &["_U", "_", "Translate", "TranslateCap", "Locales"]),
    ("@qb-core/shared/locale.lua", &["Locale"]),
    ("@qbx_core/modules/lib.lua", &["qbx"]),
    ("@qbx_core/modules/playerdata.lua", &["QBX"]),
];

fn walk(dir: &Path, manifests: &mut Vec<PathBuf>, lua: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            if !matches!(name.as_ref(), "node_modules" | ".git" | "stream" | ".vscode") {
                walk(&path, manifests, lua);
            }
        } else if name.eq_ignore_ascii_case("fxmanifest.lua") || name.eq_ignore_ascii_case("__resource.lua") {
            manifests.push(path.clone());
            lua.push(path);
        } else if name.to_ascii_lowercase().ends_with(".lua") {
            lua.push(path);
        }
    }
}

/// What a folder holds on disk: each manifest and Lua file with its size and
/// modification time. Two equal stamps mean nothing a scan reads has changed
/// (JavaScript files aside).
pub type Stamp = Vec<(PathBuf, u64, Option<std::time::SystemTime>)>;

/// The stamp of the folders, sorted by path.
pub fn stamp(folders: &[PathBuf]) -> Stamp {
    // The same files `walk` finds; sizes and times from the directory listing
    // (no file is opened).
    fn go(dir: &Path, out: &mut Stamp) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Ok(t) = entry.file_type() else { continue };
            if t.is_dir() {
                if !matches!(name.as_ref(), "node_modules" | ".git" | "stream" | ".vscode") {
                    go(&entry.path(), out);
                }
            } else if name.to_ascii_lowercase().ends_with(".lua") {
                let m = entry.metadata().ok();
                let (len, time) = m.map_or((0, None), |m| (m.len(), m.modified().ok()));
                out.push((entry.path(), len, time));
            }
        }
    }
    let mut out = Vec::new();
    for f in folders {
        go(f, &mut out);
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

struct Summary {
    globals: HashSet<String>,
    uses: HashSet<String>,
    escrow: bool,
    api: FileApi,
    /// Loads Lua code at run time (`load`, `require`, `lib.load`, `lib.require`):
    /// files its manifest does not list may still run.
    loader: bool,
}

fn summarize(text: &str) -> Summary {
    if is_escrow(text) {
        return Summary { globals: HashSet::new(), uses: HashSet::new(), escrow: true, loader: false, api: FileApi::default() };
    }
    let parse = crate::parse(text);
    let names = crate::names(&parse, text);
    let uses = global_uses(text, &names);
    let loader = uses.contains("load")
        || uses.contains("require")
        || text.contains("lib.load(")
        || text.contains("lib.require(");
    let api = crate::api::file_api(&parse, text, &names);
    Summary { globals: global_writes(&parse, text, &names).into_iter().collect(), uses, escrow: false, loader, api }
}

impl Workspace {
    /// Reads every resource and Lua file under the folders.
    pub fn scan(folders: &[PathBuf]) -> Workspace {
        Workspace::scan_with(folders, &[])
    }

    /// [`Workspace::scan`], then the resources under `library` (LuaLS's
    /// `Lua.workspace.library`, such as a copy of ox_lib or tgiann-core next to a
    /// single opened resource) that no workspace resource shares a name with: the
    /// workspace's own copy wins and the library one is not read, so it neither
    /// shadows it nor makes the name a duplicate. Lua files outside a library
    /// resource are left out.
    pub fn scan_with(folders: &[PathBuf], library: &[PathBuf]) -> Workspace {
        let mut ws = Workspace { folders: folders.to_vec(), library: library.to_vec(), ..Workspace::default() };
        let (mut manifests, mut lua) = (Vec::new(), Vec::new());
        for f in folders {
            walk(f, &mut manifests, &mut lua);
        }
        ws.add(manifests, lua);
        let (mut manifests, mut lua) = (Vec::new(), Vec::new());
        for f in library {
            walk(f, &mut manifests, &mut lua);
        }
        let mut roots: Vec<PathBuf> = manifests
            .iter()
            .filter_map(|m| m.parent())
            .filter(|r| r.file_name().is_some_and(|n| !ws.by_name.contains_key(&n.to_string_lossy().to_lowercase())))
            .map(Path::to_path_buf)
            .collect();
        roots.dedup();
        manifests.retain(|m| m.parent().is_some_and(|r| roots.iter().any(|k| k == r)));
        lua.retain(|p| roots.iter().any(|r| p.starts_with(r)));
        ws.add(manifests, lua);
        ws
    }

    fn add(&mut self, manifests: Vec<PathBuf>, lua: Vec<PathBuf>) {
        // Resources: when two manifests nest, the deepest folder owns a file.
        let parsed: Vec<(PathBuf, Manifest)> = manifests
            .par_iter()
            .filter_map(|m| {
                let text = std::fs::read_to_string(m).ok()?;
                let parse = crate::parse(&text);
                Some((m.parent()?.to_path_buf(), Manifest::parse(&parse, &text)))
            })
            .collect();
        for (root, manifest) in parsed {
            let k = key(&root);
            if self.by_root.contains_key(&k) {
                // `fxmanifest.lua` wins over `__resource.lua`; keep the first.
                continue;
            }
            let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let id = self.resources.len();
            let (js_exports, foreign) = foreign_scripts(&root, &manifest);
            self.resources.push(Resource {
                root,
                name: name.clone(),
                manifest,
                escrow: false,
                loader: false,
                files: Vec::new(),
                js_exports,
                foreign,
            });
            self.by_root.insert(k, id);
            if self.by_name.contains_key(&name.to_lowercase()) {
                self.duplicate_names.insert(name.to_lowercase());
            }
            self.by_name.entry(name.to_lowercase()).or_insert(id);
        }
        let summaries: Vec<(PathBuf, Summary)> = lua
            .par_iter()
            .filter_map(|p| {
                let bytes = std::fs::read(p).ok()?;
                Some((p.clone(), summarize(&String::from_utf8_lossy(&bytes))))
            })
            .collect();
        for (path, s) in summaries {
            self.insert(&path, s);
        }
    }

    fn resource_of(&self, path: &Path) -> Option<(usize, String)> {
        let mut dir = path.parent();
        while let Some(d) = dir {
            if let Some(&id) = self.by_root.get(&key(d)) {
                let rel = path.strip_prefix(&self.resources[id].root).ok()?.to_string_lossy().replace('\\', "/");
                return Some((id, rel));
            }
            dir = d.parent();
        }
        None
    }

    fn insert(&mut self, path: &Path, s: Summary) {
        let Summary { globals, uses, escrow, loader, api } = s;
        let k = key(path);
        if let Some(old) = self.files.remove(&k) {
            self.forget(&k, &old.api);
        }
        for n in api.classes.iter().map(|c| &c.name).chain(api.class_binds.iter().map(|(_, c)| c)) {
            let files = self.classes.entry(n.clone()).or_default();
            if !files.contains(&k) {
                files.push(k.clone());
            }
        }
        for n in api.deprecated_names() {
            *self.deprecated.entry(n.to_string()).or_insert(0) += 1;
        }
        for n in &api.doc_names {
            *self.doc_names.entry(n.clone()).or_insert(0) += 1;
        }
        for n in &api.export_events {
            *self.export_events.entry(n.clone()).or_insert(0) += 1;
        }
        for n in &api.server_bound {
            *self.server_bound.entry(n.clone()).or_insert(0) += 1;
        }
        for n in &api.fired_locally {
            *self.fired_locally.entry(n.clone()).or_insert(0) += 1;
        }
        for n in &api.event_strings {
            *self.event_strings.entry(n.clone()).or_insert(0) += 1;
        }
        let (resource, rel) = match self.resource_of(path) {
            Some((id, rel)) => (Some(id), rel),
            None => (None, String::new()),
        };
        if let Some(id) = resource {
            let r = &mut self.resources[id];
            r.escrow |= escrow;
            r.loader |= loader;
            if !r.files.contains(&k) {
                r.files.push(k.clone());
            }
        }
        self.files.insert(k, FileEntry { path: path.to_path_buf(), resource, rel, globals, uses, escrow, api });
    }

    /// A deleted file; `true` when other files' diagnostics may change. A deleted
    /// manifest rescans the workspace (resource ids would shift otherwise).
    pub fn remove(&mut self, path: &Path) -> bool {
        if is_manifest(path) {
            *self = Workspace::scan_with(&self.folders, &self.library);
            return true;
        }
        let k = key(path);
        let Some(e) = self.files.remove(&k) else { return false };
        self.forget(&k, &e.api);
        if let Some(id) = e.resource {
            self.resources[id].files.retain(|f| *f != k);
        }
        !e.globals.is_empty() || !e.uses.is_empty() || e.api != FileApi::default()
    }

    /// Drops a summary's names from the workspace-wide counts.
    fn forget(&mut self, k: &str, api: &FileApi) {
        for n in api.classes.iter().map(|c| &c.name).chain(api.class_binds.iter().map(|(_, c)| c)) {
            if let Some(files) = self.classes.get_mut(n) {
                files.retain(|f| f != k);
                if files.is_empty() {
                    self.classes.remove(n);
                }
            }
        }
        fn dec(map: &mut HashMap<String, usize>, n: &str) {
            if let Some(c) = map.get_mut(n) {
                *c -= 1;
                if *c == 0 {
                    map.remove(n);
                }
            }
        }
        for n in api.deprecated_names() {
            dec(&mut self.deprecated, n);
        }
        for n in &api.doc_names {
            dec(&mut self.doc_names, n);
        }
        for n in &api.export_events {
            dec(&mut self.export_events, n);
        }
        for n in &api.server_bound {
            dec(&mut self.server_bound, n);
        }
        for n in &api.fired_locally {
            dec(&mut self.fired_locally, n);
        }
        for n in &api.event_strings {
            dec(&mut self.event_strings, n);
        }
    }

    /// Brings a scanned workspace up to date with the disk: changed and new Lua
    /// files are read again, deleted ones dropped; a changed, new or deleted
    /// manifest rescans everything.
    pub fn refresh(&mut self, old: &Stamp, new: &Stamp) {
        let before: HashMap<&Path, (u64, Option<std::time::SystemTime>)> = old.iter().map(|(p, l, t)| (p.as_path(), (*l, *t))).collect();
        let after: HashMap<&Path, (u64, Option<std::time::SystemTime>)> = new.iter().map(|(p, l, t)| (p.as_path(), (*l, *t))).collect();
        let changed: Vec<&Path> = after.iter().filter(|(p, s)| before.get(*p) != Some(*s)).map(|(p, _)| *p).collect();
        let gone: Vec<&Path> = before.keys().filter(|p| !after.contains_key(*p)).copied().collect();
        if changed.iter().chain(&gone).any(|p| is_manifest(p)) {
            *self = Workspace::scan_with(&self.folders, &self.library);
            return;
        }
        for p in gone {
            self.remove(p);
        }
        for p in changed {
            if let Ok(bytes) = std::fs::read(p) {
                self.update(p, &String::from_utf8_lossy(&bytes));
            }
        }
    }

    /// A file outside the workspace folders: loads the resource around it once.
    pub fn ensure(&mut self, path: &Path) {
        if self.files.contains_key(&key(path)) {
            return;
        }
        let mut dir = path.parent();
        while let Some(d) = dir {
            if self.by_root.contains_key(&key(d)) {
                break;
            }
            if d.join("fxmanifest.lua").is_file() || d.join("__resource.lua").is_file() {
                let (mut manifests, mut lua) = (Vec::new(), Vec::new());
                walk(d, &mut manifests, &mut lua);
                self.add(manifests, lua);
                break;
            }
            dir = d.parent();
        }
    }

    /// A file's new text (an open document): its globals, or the manifest.
    /// `true` when other files' diagnostics may change (new globals, a manifest).
    pub fn update(&mut self, path: &Path, text: &str) -> bool {
        let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        if (name == "fxmanifest.lua" || name == "__resource.lua")
            && let Some(dir) = path.parent()
        {
            let parse = crate::parse(text);
            let manifest = Manifest::parse(&parse, text);
            match self.by_root.get(&key(dir)) {
                Some(&id) => {
                    let r = &mut self.resources[id];
                    (r.js_exports, r.foreign) = foreign_scripts(&r.root, &manifest);
                    r.manifest = manifest;
                }
                None => {
                    // A new resource: rescan its folder.
                    let (mut manifests, mut lua) = (Vec::new(), Vec::new());
                    walk(dir, &mut manifests, &mut lua);
                    self.add(manifests, lua);
                }
            }
            self.insert(path, summarize(text));
            return true;
        }
        let s = summarize(text);
        let changed = self
            .files
            .get(&key(path))
            .is_none_or(|e| e.globals != s.globals || e.uses != s.uses || e.escrow != s.escrow || !e.api.same_shape(&s.api));
        self.insert(path, s);
        changed
    }

    pub fn info(&self, path: &Path) -> FileInfo {
        let entry = self.files.get(&key(path));
        let Some((r, rel)) = entry.and_then(|e| Some((&self.resources[e.resource?], e.rel.as_str()))) else {
            return FileInfo {
                side: FileSide::Unknown,
                game: Game::Gta5,
                resource: None,
                ox_lib: false,
                globals_unknown: false,
                manifest: is_manifest(path),
                loaded: true,
            };
        };
        let side = r.manifest.side(rel);
        let imports_unknown = self.imports(r, side).iter().any(|i| i.is_none());
        FileInfo {
            side,
            game: r.manifest.game(),
            resource: Some(r.name.clone()),
            ox_lib: r.manifest.loads_ox_lib(side) || r.name.eq_ignore_ascii_case("ox_lib"),
            globals_unknown: r.escrow || r.manifest.dynamic || imports_unknown,
            manifest: is_manifest(path),
            loaded: is_manifest(path) || r.loader || r.manifest.loads(rel) || self.imported(&r.name, rel),
        }
    }

    /// Whether a type the file names may be declared where the index cannot
    /// read it: its own resource has escrow files, a dynamic manifest or an
    /// import outside the workspace, or a resource it depends on or imports from
    /// has escrow files (tgiann-core's classes live in its escrow files on some
    /// servers).
    pub(crate) fn doc_types_hidden(&self, path: &Path) -> bool {
        if self.info(path).globals_unknown {
            return true;
        }
        let Some(r) = self.files.get(&key(path)).and_then(|e| e.resource) else { return false };
        self.related(r).into_iter().any(|i| self.resources[i].escrow)
    }

    /// The `---@alias` / `---@enum` types the other files of a file's resource
    /// declare, then those of the resources its manifest names; the first of a
    /// name wins.
    pub(crate) fn visible_aliases(&self, path: &Path) -> Vec<(String, crate::luacats::DocType)> {
        let k = key(path);
        let Some(r) = self.files.get(&k).and_then(|e| e.resource) else { return Vec::new() };
        let mut out: Vec<(String, crate::luacats::DocType)> = Vec::new();
        for res in std::iter::once(r).chain(self.related(r)) {
            for f in self.resources[res].files.iter().filter(|f| **f != k) {
                for (n, t) in self.files.get(f).map(|e| e.api.aliases.as_slice()).unwrap_or_default() {
                    if !out.iter().any(|(o, _)| o == n) {
                        out.push((n.clone(), t.clone()));
                    }
                }
            }
        }
        out
    }

    /// The `---@class`es a file may use from the resource's files, then from the
    /// resources its manifest names: those its doc comments, `aliases` and the
    /// signatures of `calls` name, with the classes their fields and parents name in turn. A name
    /// belongs to the first resource declaring it, and its declarations there
    /// are merged (as in one file). The file's own classes win over these.
    pub(crate) fn visible_classes(
        &self,
        path: &Path,
        text: &str,
        aliases: &[(String, crate::luacats::DocType)],
        calls: &[(String, Vec<crate::luacats::FunType>)],
    ) -> Vec<crate::api::ClassDef> {
        let k = key(path);
        let Some(r) = self.files.get(&k).and_then(|e| e.resource) else { return Vec::new() };
        let resources: Vec<usize> = std::iter::once(r).chain(self.related(r)).collect();
        let mut todo: Vec<String> = Vec::new();
        for line in text.lines().map(str::trim_start).filter(|l| l.starts_with("---")) {
            doc_words(line, &mut todo);
        }
        for (_, t) in aliases {
            type_names(t, &mut todo);
        }
        for f in calls.iter().flat_map(|(_, fs)| fs) {
            type_names(&crate::luacats::DocType::Fun(Box::new(f.clone())), &mut todo);
        }
        if todo.is_empty() {
            return Vec::new();
        }
        // Most doc words are prose: look up only names some file declares.
        let declared: HashSet<&str> = resources
            .iter()
            .flat_map(|&res| self.resources[res].files.iter())
            .filter_map(|f| self.files.get(f))
            .flat_map(|e| e.api.classes.iter().map(|c| c.name.as_str()))
            .collect();
        let mut seen: HashSet<String> = HashSet::new();
        let mut out: Vec<crate::api::ClassDef> = Vec::new();
        while let Some(name) = todo.pop() {
            if !declared.contains(name.as_str()) || !seen.insert(name.clone()) {
                continue;
            }
            let Some(c) = resources.iter().find_map(|&res| self.class_in(res, &name)) else { continue };
            todo.extend(c.parents.iter().cloned());
            for f in &c.fields {
                type_names(&f.ty, &mut todo);
            }
            for (key, value) in &c.indexes {
                type_names(key, &mut todo);
                type_names(value, &mut todo);
            }
            out.push(c);
        }
        out
    }

    /// A resource's `---@class name`, its declarations in every file merged.
    /// The resource whose `---@class name` a file of resource `r` sees: its
    /// own, else the first resource its manifest names that declares it.
    pub(crate) fn class_home(&self, r: usize, name: &str) -> Option<usize> {
        std::iter::once(r).chain(self.related(r)).find(|&res| {
            self.resources[res].files.iter().any(|f| self.files.get(f).is_some_and(|e| e.api.classes.iter().any(|c| c.name == name)))
        })
    }

    fn class_in(&self, res: usize, name: &str) -> Option<crate::api::ClassDef> {
        let mut found: Option<crate::api::ClassDef> = None;
        for f in &self.resources[res].files {
            for c in self.files.get(f).map(|e| e.api.classes.as_slice()).unwrap_or_default().iter().filter(|c| c.name == name) {
                match found.as_mut() {
                    Some(have) => {
                        have.fields.extend(c.fields.iter().cloned());
                        have.parents.extend(c.parents.iter().cloned());
                        have.parent_types.extend(c.parent_types.iter().cloned());
                        have.operators.extend(c.operators.iter().cloned());
                        have.indexes.extend(c.indexes.iter().cloned());
                        have.exact |= c.exact;
                        have.partial |= c.partial;
                    }
                    None => found = Some(c.clone()),
                }
            }
        }
        found
    }

    /// The resources a resource's manifest names: `dependency` entries and the
    /// resources of its `@res/file.lua` imports, in the workspace.
    pub(crate) fn related(&self, res: usize) -> Vec<usize> {
        let m = &self.resources[res].manifest;
        let imported = [&m.shared, &m.client, &m.server]
            .into_iter()
            .flatten()
            .filter_map(|p| p.strip_prefix('@'))
            .filter_map(|p| p.split('/').next());
        let mut out: Vec<usize> = Vec::new();
        for i in m.dependencies.iter().map(String::as_str).chain(imported).filter_map(|n| self.by_name.get(&n.to_lowercase()).copied()) {
            if i != res && !out.contains(&i) {
                out.push(i);
            }
        }
        out
    }

    /// Whether a manifest imports the file (`@resource/rel`).
    fn imported(&self, resource: &str, rel: &str) -> bool {
        let prefix = format!("@{}/", resource.to_lowercase());
        self.resources.iter().any(|r| {
            let m = &r.manifest;
            [&m.shared, &m.client, &m.server].into_iter().flatten().any(|p| {
                p.get(..prefix.len()).is_some_and(|h| h.eq_ignore_ascii_case(&prefix))
                    && p.get(prefix.len()..).is_some_and(|rest| !rest.is_empty() && crate::manifest::glob(rest, rel))
            })
        })
    }

    /// The manifest's `@res/file` entries for a side: the globals each one
    /// defines, `None` when it is neither in the workspace nor a known file.
    fn imports(&self, r: &Resource, side: FileSide) -> Vec<Option<HashSet<String>>> {
        let m = &r.manifest;
        let lists: Vec<&Vec<String>> = match side {
            FileSide::Client => vec![&m.shared, &m.client],
            FileSide::Server => vec![&m.shared, &m.server],
            _ => vec![&m.shared, &m.client, &m.server],
        };
        lists
            .into_iter()
            .flatten()
            .filter(|p| p.starts_with('@'))
            .map(|p| {
                let lower = p.to_lowercase();
                let known: Option<HashSet<String>> = KNOWN_IMPORTS
                    .iter()
                    .find(|(f, _)| *f == lower)
                    .map(|(_, names)| names.iter().map(|n| n.to_string()).collect());
                match (known, self.import_globals(p)) {
                    (Some(mut k), Some(w)) => {
                        k.extend(w);
                        Some(k)
                    }
                    (k, w) => k.or(w),
                }
            })
            .collect()
    }

    /// The globals of the workspace's copy of an `@res/file` entry; `None` when it
    /// is missing or escrow (its globals cannot be read).
    fn import_globals(&self, entry: &str) -> Option<HashSet<String>> {
        let (res, file) = entry[1..].split_once('/')?;
        let id = *self.by_name.get(&res.to_lowercase())?;
        let rr = &self.resources[id];
        if file.contains('*') {
            // A glob over another resource: every matching file.
            let mut all = HashSet::new();
            for f in &rr.files {
                if let Some(e) = self.files.get(f)
                    && crate::manifest::glob(file, &e.rel)
                {
                    if e.escrow {
                        return None;
                    }
                    all.extend(e.globals.iter().cloned());
                }
            }
            return Some(all);
        }
        self.files.get(&key(&rr.root.join(file))).filter(|e| !e.escrow).map(|e| e.globals.clone())
    }

    /// Whether another file the file can see defines the global.
    pub fn defined_elsewhere(&self, path: &Path, name: &str) -> bool {
        self.in_other_files(path, |e| e.globals.contains(name)).unwrap_or(false)
            || self.import_defines(path, name)
    }

    /// Whether another file that shares the file's Lua state reads the global (a
    /// lowercase global used across files is meant to be global). Escrow files can
    /// read anything, so a resource with them counts as using every global.
    pub fn used_elsewhere(&self, path: &Path, name: &str) -> bool {
        let escrow = self.files.get(&key(path)).and_then(|e| e.resource).is_some_and(|id| self.resources[id].escrow);
        escrow || self.in_other_files(path, |e| e.uses.contains(name)).unwrap_or(false)
    }

    fn import_defines(&self, path: &Path, name: &str) -> bool {
        let Some(entry) = self.files.get(&key(path)) else { return false };
        let Some(id) = entry.resource else { return false };
        let r = &self.resources[id];
        self.imports(r, r.manifest.side(&entry.rel)).iter().flatten().any(|g| g.contains(name))
    }

    /// Whether a file sharing a Lua state with the file (same resource and side,
    /// shared files on both; loose files outside resources with each other)
    /// matches. `None` for an unknown file.
    fn in_other_files(&self, path: &Path, pred: impl Fn(&FileEntry) -> bool) -> Option<bool> {
        let k = key(path);
        let entry = self.files.get(&k)?;
        let Some(id) = entry.resource else {
            return Some(self.files.iter().any(|(fk, e)| e.resource.is_none() && *fk != k && pred(e)));
        };
        let r = &self.resources[id];
        let side = r.manifest.side(&entry.rel);
        let visible = |other: FileSide| match side {
            FileSide::Client => matches!(other, FileSide::Client | FileSide::Shared | FileSide::Unknown),
            FileSide::Server => matches!(other, FileSide::Server | FileSide::Shared | FileSide::Unknown),
            FileSide::Shared | FileSide::Unknown => true,
        };
        Some(r.files.iter().any(|f| {
            *f != k
                && self.files.get(f).is_some_and(|e| {
                    pred(e) && (r.manifest.dynamic || visible(r.manifest.side(&e.rel)))
                })
        }))
    }

    pub fn folders(&self) -> &[PathBuf] {
        &self.folders
    }

    /// Every known Lua file that is not escrow, for `check`.
    pub fn lua_files(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = self.files.values().filter(|e| !e.escrow).map(|e| e.path.clone()).collect();
        out.sort();
        out
    }

    /// The resource folder and manifest of a file, for tools that describe it.
    pub fn resource(&self, path: &Path) -> Option<(&Path, &str, &Manifest)> {
        let r = &self.resources[self.files.get(&key(path))?.resource?];
        Some((&r.root, &r.name, &r.manifest))
    }

    /// The globals a file defines.
    pub fn globals_of(&self, path: &Path) -> Vec<String> {
        let mut g: Vec<String> = self.files.get(&key(path)).map(|e| e.globals.iter().cloned().collect()).unwrap_or_default();
        g.sort();
        g
    }

    pub fn resource_count(&self) -> usize {
        self.resources.len()
    }
}

const MANIFEST_RULES: &[&str] = &[
    "unused-local",
    "unused-vararg",
    "unused-label",
    "redefined-local",
    "trailing-space",
    "empty-block",
    "code-after-break",
    "unbalanced-assignments",
    "redundant-value",
    "duplicate-index",
    "newline-call",
];

/// The files under a resource folder, relative with `/`, in their real case.
fn resource_files(root: &Path) -> Vec<String> {
    const MAX: usize = 50_000;
    fn go(dir: &Path, rel: &str, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            if out.len() >= MAX {
                return;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let r = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                if !matches!(name.as_str(), "node_modules" | ".git") {
                    go(&entry.path(), &r, out);
                }
            } else {
                out.push(r);
            }
        }
    }
    let mut out = Vec::new();
    go(root, "", &mut out);
    out
}

impl Workspace {
    /// A manifest's entries checked against the disk: missing scripts, a
    /// different letter case, globs matching nothing, `@res/file` imports of a
    /// workspace resource that has no such file.
    fn manifest_lints(&self, path: &Path, parse: &Parse, text: &str, config: &LintConfig) -> Vec<crate::lint::Lint> {
        use crate::manifest::EntryKind;
        let m = Manifest::parse(parse, text);
        let Some(root) = path.parent() else { return Vec::new() };
        let mut out = Vec::new();
        let mut push = |code: &'static str, range: crate::TextRange, message: String| {
            if let Some(l) = crate::lint::make(code, range, message, None, config) {
                out.push(l);
            }
        };
        let mut files: Option<Vec<String>> = None;
        let mut imported: HashMap<String, Option<Vec<String>>> = HashMap::new();
        for e in &m.entries {
            let v = e.value.trim().trim_start_matches("./");
            if v.is_empty() || v.ends_with('/') || v.contains("://") || v.contains('\\') {
                continue;
            }
            // `@res/path`: another resource's file.
            if let Some(rest) = v.strip_prefix('@') {
                let Some((res, file)) = rest.split_once('/') else { continue };
                if e.kind == EntryKind::Files || file.contains('*') || file.contains('?') {
                    continue;
                }
                let list = imported.entry(res.to_ascii_lowercase()).or_insert_with(|| {
                    let id = self.by_name.get(&res.to_ascii_lowercase())?;
                    Some(resource_files(&self.resources[*id].root))
                });
                let Some(list) = list else { continue };
                if !list.iter().any(|f| f.eq_ignore_ascii_case(file)) {
                    push("fivem/manifest-missing-import", e.range, format!("`{res}` has no file `{file}`."));
                }
                continue;
            }
            // `resource_files` does not list `node_modules`: such entries are
            // checked on the disk itself, globs into it not at all.
            if v.split('/').any(|part| part == "node_modules") {
                let plain = Path::new(v).components().all(|c| matches!(c, std::path::Component::Normal(_)));
                if plain && !v.contains('*') && !v.contains('?') && !root.join(v).exists() {
                    push("fivem/manifest-missing-file", e.range, format!("`{v}` does not exist in the resource."));
                }
                continue;
            }
            let files = files.get_or_insert_with(|| resource_files(root));
            if v.contains('*') || v.contains('?') {
                if !files.iter().any(|f| crate::manifest::glob(v, f)) {
                    push("fivem/manifest-no-match", e.range, format!("`{v}` matches no file of the resource."));
                }
                continue;
            }
            if files.iter().any(|f| f == v) {
                continue;
            }
            if let Some(real) = files.iter().find(|f| f.eq_ignore_ascii_case(v)) {
                push(
                    "fivem/manifest-path-case",
                    e.range,
                    format!("The file is `{real}`: Windows finds it, a Linux server (case-sensitive file names) does not."),
                );
                continue;
            }
            match e.kind {
                EntryKind::Files => push("fivem/manifest-no-match", e.range, format!("`{v}` matches no file of the resource.")),
                _ => push("fivem/manifest-missing-file", e.range, format!("`{v}` does not exist in the resource.")),
            }
        }
        out
    }
}

pub fn is_manifest(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|n| n.eq_ignore_ascii_case("fxmanifest.lua") || n.eq_ignore_ascii_case("__resource.lua"))
}

/// Lints a file in the workspace.
pub fn lint_file(
    ws: &Workspace,
    path: &Path,
    parse: &Parse,
    text: &str,
    names: &Names,
    config: &LintConfig,
) -> Vec<crate::lint::Lint> {
    let info = ws.info(path);
    let defined = |n: &str| ws.defined_elsewhere(path, n);
    let used = |n: &str| ws.used_elsewhere(path, n);
    let aliases = ws.visible_aliases(path);
    // The file's current summary (the index may hold an older copy).
    let overlay = crate::api::file_api(parse, text, names);
    let calls = if info.manifest { Vec::new() } else { ws.ext_calls(path, &overlay, text, names, parse) };
    let classes = ws.visible_classes(path, text, &aliases, &calls);
    let ctx = Context {
        side: info.side,
        game: info.game,
        resource: info.resource.as_deref(),
        ox_lib: info.ox_lib,
        defined_elsewhere: &defined,
        used_elsewhere: &used,
        globals_unknown: info.globals_unknown,
        aliases: &aliases,
        classes: &classes,
        calls: &calls,
        config,
    };
    let mut lints = crate::lint::lint(parse, text, names, &ctx);
    if info.manifest {
        // A manifest's calls are metadata directives in an environment of their own:
        // only the rules about the Lua itself apply, and the manifest's own.
        let mut lints: Vec<_> = lints.into_iter().filter(|l| MANIFEST_RULES.contains(&l.code)).collect();
        let extra = ws.manifest_lints(path, parse, text, config);
        if !extra.is_empty() {
            lints.extend(crate::lint::suppress(parse, text, extra));
            lints.sort_by_key(|d| (d.range.start(), d.range.end()));
        }
        return lints;
    }
    let extra = ws.api_lints(path, parse, text, names, &overlay, config);
    if !extra.is_empty() {
        lints.extend(crate::lint::suppress(parse, text, extra));
        lints.sort_by_key(|d| (d.range.start(), d.range.end()));
    }
    lints
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, rel: &str, text: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn sides_globals_and_imports() {
        let root = std::env::temp_dir().join(format!("cfx-ws-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        write(&root, "lib_res/shared/util.lua", "Util = {}\n");
        write(&root, "lib_res/fxmanifest.lua", "shared_script 'shared/util.lua'\n");
        write(
            &root,
            "app/fxmanifest.lua",
            "shared_scripts { '@ox_lib/init.lua', '@lib_res/shared/util.lua', 'config.lua' }\nclient_script 'client.lua'\nserver_script 'server.lua'\n",
        );
        write(&root, "app/config.lua", "Config = {}\n_G.Shared = 1\n");
        write(&root, "app/client.lua", "ClientOnly = 1\nprint(Config, Util, Shared, lib)\n");
        write(&root, "app/server.lua", "print(ClientOnly)\nlocal function f() return helper end\nprint(f)\n");
        write(&root, "app/s2.lua", "function helper() end\nfunction alone() end\n");
        write(&root, "loose/a.lua", "Loose = 1\n");
        let ws = Workspace::scan(std::slice::from_ref(&root));
        assert_eq!(ws.resource_count(), 2);
        let client = root.join("app/client.lua");
        let server = root.join("app/server.lua");
        let info = ws.info(&client);
        assert_eq!((info.side, info.ox_lib, info.globals_unknown), (FileSide::Client, true, false));
        assert_eq!(info.resource.as_deref(), Some("app"));
        assert!(ws.defined_elsewhere(&client, "Config"));
        assert!(ws.defined_elsewhere(&client, "Shared"));
        assert!(ws.defined_elsewhere(&client, "Util"));
        assert!(ws.defined_elsewhere(&client, "lib"));
        // A client global is not visible on the server.
        assert!(!ws.defined_elsewhere(&server, "ClientOnly"));
        let text = std::fs::read_to_string(&server).unwrap();
        let p = crate::parse(&text);
        let n = crate::names(&p, &text);
        let lints = lint_file(&ws, &server, &p, &text, &n, &LintConfig::default());
        assert!(lints.iter().any(|l| l.code == "undefined-global" && &text[l.range] == "ClientOnly"));
        // `helper` is read by server.lua: global on purpose; `alone` is not.
        let s2 = root.join("app/s2.lua");
        let text = std::fs::read_to_string(&s2).unwrap();
        let p = crate::parse(&text);
        let n = crate::names(&p, &text);
        let lints = lint_file(&ws, &s2, &p, &text, &n, &LintConfig::default());
        let lower: Vec<&str> = lints.iter().filter(|l| l.code == "lowercase-global").map(|l| &text[l.range]).collect();
        assert_eq!(lower, ["alone"]);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn classes_of_other_files() {
        let root = std::env::temp_dir().join(format!("cfx-wscls-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        write(&root, "lib/fxmanifest.lua", "shared_script 'types.lua'\n");
        write(&root, "lib/types.lua", "---@class Theme\n---@field color string\n---@class Opts\n---@field other number\n");
        write(&root, "app/fxmanifest.lua", "dependency 'lib'\nshared_scripts { 'shared/type.lua', 'shared/more.lua' }\nclient_script 'client.lua'\n");
        write(
            &root,
            "app/shared/type.lua",
            "---@class Opts\n---@field id integer\n---@field inner Inner\n---@class Inner\n---@field on boolean\n---@class (partial) Part: Opts\n---@field extra? string\n",
        );
        // A class declared in two files of the resource is one class.
        write(&root, "app/shared/more.lua", "---@class Opts\n---@field name string\n");
        let text = "---@type Opts\nlocal a = { id = 1, name = 'x', inner = { on = true } }\n---@type Opts\nlocal b = { id = 1 }\n---@type Opts\nlocal c = { id = 'x', name = 'n', inner = { on = true } }\n---@type Part\nlocal d = { extra = 'e' }\n---@type Opts\nlocal e = { id = 1, name = 'n', inner = { on = 5 } }\n---@type Theme\nlocal f = {}\na.later = 1\nprint(a, b, c, d, e, f, a.method)\n";
        write(&root, "app/client.lua", text);
        let ws = Workspace::scan(std::slice::from_ref(&root));
        let path = root.join("app/client.lua");
        let p = crate::parse(text);
        let n = crate::names(&p, text);
        let all = lint_file(&ws, &path, &p, text, &n, &LintConfig::default());
        let line = |l: &crate::lint::Lint| text[..usize::from(l.range.start())].matches('\n').count() + 1;
        let found: Vec<(usize, &str)> = all.iter().filter(|l| !matches!(l.code, "unused-local" | "lowercase-global")).map(|l| (line(l), l.code)).collect();
        // The resource's own `Opts` wins over lib's; lib's `Theme` is known too;
        // `a.later` and `a.method` are not called undefined or injected.
        assert_eq!(found, [(4, "missing-fields"), (6, "assign-type-mismatch"), (10, "assign-type-mismatch"), (12, "missing-fields")], "{found:?}");
        let b = all.iter().find(|l| line(l) == 4).map(|l| l.message.as_str()).unwrap_or_default();
        assert!(b.contains("`name`") && b.contains("`inner`") && !b.contains("`other`"), "{b}");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn calls_to_functions_of_other_files() {
        let root = std::env::temp_dir().join(format!("cfx-wscall-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        write(&root, "app/fxmanifest.lua", "client_scripts { 'contract.lua', 'ui.lua', 'main.lua' }\n");
        write(
            &root,
            "app/contract.lua",
            "---@class Msg.Open\n---@field id integer\n---@field name string\n---@alias Action 'open'|'hide'\n---@overload fun(action: 'open', data: Msg.Open)\n---@overload fun(action: 'hide')\n---@param action Action\n---@param data? any\nfunction NuiSend(action, data) end\n---@param o Msg.Open\nfunction SendOpts(o) end\nObj = {}\n---@param n integer\nfunction Obj:add(n) end\nfunction Loose(a) end\n",
        );
        // The NUI contract pattern: the function reaches the callers through a field.
        write(&root, "app/ui.lua", "UiUtility = {}\nUiUtility.sendNui = NuiSend\n");
        let text = "UiUtility.sendNui('open', { id = 1 })\nUiUtility.sendNui('open', { id = 'x', name = 'n' })\nUiUtility.sendNui('bad')\nNuiSend('open', { id = 1, name = 'n' })\nNuiSend('hide')\nSendOpts({ id = 2 })\nObj:add('x')\nObj.add(Obj, 3)\nLoose({})\n";
        write(&root, "app/main.lua", text);
        let ws = Workspace::scan(std::slice::from_ref(&root));
        let path = root.join("app/main.lua");
        let p = crate::parse(text);
        let n = crate::names(&p, text);
        let all = lint_file(&ws, &path, &p, text, &n, &LintConfig::default());
        let line = |l: &crate::lint::Lint| text[..usize::from(l.range.start())].matches('\n').count() + 1;
        let found: Vec<(usize, &str)> = all.iter().map(|l| (line(l), l.code)).collect();
        assert_eq!(
            found,
            [(1, "missing-fields"), (2, "assign-type-mismatch"), (3, "param-type-mismatch"), (6, "missing-fields"), (7, "param-type-mismatch")],
            "{all:?}"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn calls_to_other_files_keep_their_own_names() {
        let root = std::env::temp_dir().join(format!("cfx-wscall2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // `Opts` of lib is not the app's `Opts`.
        write(&root, "lib/fxmanifest.lua", "shared_script 'types.lua'\n");
        write(&root, "lib/types.lua", "---@class Opts\n---@field other number\n---@param o Opts\nfunction LibSend(o) end\n");
        // The file runs in app too (`@lib/types.lua`).
        write(&root, "app/fxmanifest.lua", "shared_scripts { '@lib/types.lua', 'shared.lua' }\nclient_script 'client.lua'\n");
        // A doc block above a blank line is not the function's.
        write(&root, "app/shared.lua", "---@class Opts\n---@field id integer\n---@param a string\n\nfunction Loose(a) end\n---@param n integer\nfunction Num(n) end\n");
        let text = "LibSend({ other = 1 })\nLoose(1)\nlocal function f(Num) Num('x') end\nf(print)\nNum('y')\n";
        write(&root, "app/client.lua", text);
        let ws = Workspace::scan(std::slice::from_ref(&root));
        let path = root.join("app/client.lua");
        let p = crate::parse(text);
        let n = crate::names(&p, text);
        let all = lint_file(&ws, &path, &p, text, &n, &LintConfig::default());
        let line = |l: &crate::lint::Lint| text[..usize::from(l.range.start())].matches('\n').count() + 1;
        let found: Vec<(usize, &str)> = all.iter().map(|l| (line(l), l.code)).collect();
        // Only the global `Num` (line 5); the parameter `Num` shadows it on line 3.
        assert_eq!(found, [(5, "param-type-mismatch")], "{all:?}");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
