//! `fxmanifest.lua` / `__resource.lua`: which files a resource loads on which side,
//! read from the syntax tree. Only literal arguments count; a directive with any
//! other argument makes the manifest dynamic, and side rules stay silent for the
//! whole resource (its file lists are not known).

use crate::library::Game;
use crate::{Parse, TextRange};
use cfx_lua_syntax::SyntaxKind::*;
use cfx_lua_syntax::SyntaxNode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileSide {
    Client,
    Server,
    Shared,
    /// Not listed (or the manifest is dynamic): no side rules.
    Unknown,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Manifest {
    pub client: Vec<String>,
    pub server: Vec<String>,
    pub shared: Vec<String>,
    /// `games { ... }` / `game '...'`, lower case.
    pub games: Vec<String>,
    pub dependencies: Vec<String>,
    /// `files { ... }`: sent to clients, read with `LoadResourceFile` or ox_lib's
    /// `require`.
    pub files: Vec<String>,
    /// `export` / `exports` (client) and `server_export(s)`: global functions the
    /// resource exports by name (the old way; `exports('name', fn)` is the new).
    pub client_exports: Vec<String>,
    pub server_exports: Vec<String>,
    /// `provide 'name'`: the resource answers for another resource's name
    /// (qbx_core provides `qb-core`).
    pub provides: Vec<String>,
    /// Every script list, `files` and `ui_page` string with its range, for the
    /// manifest's own diagnostics.
    pub entries: Vec<Entry>,
    /// A script list holds something other than string literals.
    pub dynamic: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Client,
    Server,
    Shared,
    Files,
    UiPage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub kind: EntryKind,
    pub value: String,
    /// The string literal.
    pub range: TextRange,
}

/// Script directives and the list they feed.
fn list_of(directive: &str) -> Option<u8> {
    match directive {
        "client_script" | "client_scripts" => Some(0),
        "server_script" | "server_scripts" => Some(1),
        "shared_script" | "shared_scripts" => Some(2),
        "game" | "games" => Some(3),
        "dependency" | "dependencies" => Some(4),
        "file" | "files" => Some(5),
        "export" | "exports" => Some(6),
        "server_export" | "server_exports" => Some(7),
        "provide" => Some(8),
        "ui_page" => Some(9),
        _ => None,
    }
}

impl Manifest {
    pub fn parse(parse: &Parse, text: &str) -> Manifest {
        let mut m = Manifest::default();
        let root = parse.syntax();
        let Some(block) = root.children().find(|c| c.kind() == BLOCK) else { return m };
        for stat in block.children().filter(|s| s.kind() == CALL_STAT) {
            let Some(call) = stat.first_child() else { continue };
            // `client_scripts { ... }`, `client_script 'a' 'b'` (a chain of calls).
            let mut calls = vec![call];
            while let Some(inner) = calls.last().and_then(|c| c.first_child()).filter(|c| c.kind() == CALL_EXPR) {
                calls.push(inner);
            }
            let Some(head) = calls.last().and_then(|c| c.first_child()).filter(|c| c.kind() == NAME_EXPR) else { continue };
            let Some(list) = list_of(&text[head.text_range()]) else { continue };
            let mut values = Vec::new();
            let mut dynamic = false;
            for call in calls.iter().rev() {
                let Some(args) = call.children().find(|c| c.kind() == ARG_LIST) else { continue };
                strings(&args, text, &mut values, &mut dynamic);
            }
            if list <= 2 && dynamic {
                m.dynamic = true;
            }
            let kind = match list {
                0 => Some(EntryKind::Client),
                1 => Some(EntryKind::Server),
                2 => Some(EntryKind::Shared),
                5 => Some(EntryKind::Files),
                9 => Some(EntryKind::UiPage),
                _ => None,
            };
            if let Some(kind) = kind {
                m.entries.extend(values.iter().map(|(v, r)| Entry { kind, value: v.clone(), range: *r }));
            }
            let target = match list {
                0 => &mut m.client,
                1 => &mut m.server,
                2 => &mut m.shared,
                3 => &mut m.games,
                4 => &mut m.dependencies,
                5 => &mut m.files,
                6 => &mut m.client_exports,
                7 => &mut m.server_exports,
                8 => &mut m.provides,
                _ => continue,
            };
            target.extend(values.into_iter().map(|(v, _)| v));
        }
        for g in &mut m.games {
            *g = g.to_ascii_lowercase();
        }
        m
    }

    /// The game whose natives apply: RDR3 only when the manifest lists it and not GTA V.
    pub fn game(&self) -> Game {
        let rdr = self.games.iter().any(|g| g == "rdr3");
        let gta = self.games.iter().any(|g| g == "gta5" || g == "common");
        if rdr && !gta { Game::Rdr3 } else { Game::Gta5 }
    }

    /// The side a file loads on, by its path relative to the resource folder
    /// (`/` separators).
    pub fn side(&self, rel: &str) -> FileSide {
        if self.dynamic {
            return FileSide::Unknown;
        }
        let hit = |list: &[String]| list.iter().any(|p| !p.starts_with('@') && glob(p, rel));
        let (c, s) = (hit(&self.client), hit(&self.server));
        if hit(&self.shared) || (c && s) {
            FileSide::Shared
        } else if c {
            FileSide::Client
        } else if s {
            FileSide::Server
        } else {
            FileSide::Unknown
        }
    }

    /// Whether the resource can run the file: a script list or `files` names it,
    /// or the lists are not known.
    pub fn loads(&self, rel: &str) -> bool {
        self.dynamic || self.side(rel) != FileSide::Unknown || self.files.iter().any(|p| glob(p, rel))
    }

    /// Whether `@ox_lib/init.lua` is loaded for files of that side.
    pub fn loads_ox_lib(&self, side: FileSide) -> bool {
        let has = |list: &[String]| list.iter().any(|p| p.eq_ignore_ascii_case("@ox_lib/init.lua"));
        let shared = has(&self.shared);
        match side {
            FileSide::Client => shared || has(&self.client),
            FileSide::Server => shared || has(&self.server),
            FileSide::Shared => shared || (has(&self.client) && has(&self.server)),
            FileSide::Unknown => false,
        }
    }
}

/// String literals in an argument list or table constructor; anything else is dynamic.
/// A string argument without parentheses (`f 'x'`) is a token of the argument list.
fn strings(node: &SyntaxNode, text: &str, out: &mut Vec<(String, TextRange)>, dynamic: &mut bool) {
    for child in node.children_with_tokens() {
        match child {
            rowan::NodeOrToken::Token(t) => {
                if matches!(t.kind(), STRING | LONG_STRING) {
                    match literal(&t, text) {
                        Some(s) => out.push((s, t.text_range())),
                        None => *dynamic = true,
                    }
                }
            }
            rowan::NodeOrToken::Node(child) => match child.kind() {
                LITERAL_EXPR => match child.first_token().and_then(|t| Some((literal(&t, text)?, t.text_range()))) {
                    Some(s) => out.push(s),
                    None => *dynamic = true,
                },
                TABLE_EXPR | FIELD_POS => strings(&child, text, out, dynamic),
                _ => *dynamic = true,
            },
        }
    }
}

fn literal(t: &cfx_lua_syntax::SyntaxToken, text: &str) -> Option<String> {
    let s = &text[t.text_range()];
    match t.kind() {
        STRING => Some(crate::util::unquote(s)?.replace('\\', "/")),
        LONG_STRING => {
            let open = s.find('[').and_then(|i| s[i + 1..].find('[').map(|j| i + j + 2))?;
            let close = s.rfind(']').and_then(|e| s[..e].rfind(']'))?;
            (open <= close).then(|| s[open..close].to_string())
        }
        _ => None,
    }
}

/// FiveM's manifest globs: `*` within a path segment, `**` across segments,
/// matched without regard to case (Windows servers).
pub fn glob(pattern: &str, path: &str) -> bool {
    let p = pattern.trim_start_matches("./").to_ascii_lowercase();
    let s = path.to_ascii_lowercase();
    glob_bytes(p.as_bytes(), s.as_bytes())
}

/// The same globs, case-sensitive (LuaLS's `doc.privateName` names).
pub fn glob_exact(pattern: &str, name: &str) -> bool {
    glob_bytes(pattern.as_bytes(), name.as_bytes())
}

/// Memoized over (pattern index, path index), so a pattern full of wildcards
/// stays polynomial.
fn glob_bytes(p: &[u8], s: &[u8]) -> bool {
    fn go(p: &[u8], s: &[u8], pi: usize, si: usize, memo: &mut [Option<bool>]) -> bool {
        let slot = pi * (s.len() + 1) + si;
        if let Some(v) = memo[slot] {
            return v;
        }
        let (pp, ss) = (&p[pi..], &s[si..]);
        let v = if pp.starts_with(b"**") {
            // `**/` also matches no folder at all.
            (pp.get(2) == Some(&b'/') && go(p, s, pi + 3, si, memo)) || (si..=s.len()).any(|i| go(p, s, pi + 2, i, memo))
        } else {
            match pp.first() {
                None => ss.is_empty(),
                Some(b'*') => {
                    let segment = ss.iter().position(|&c| c == b'/').unwrap_or(ss.len());
                    (si..=si + segment).any(|i| go(p, s, pi + 1, i, memo))
                }
                Some(b'?') => ss.first().is_some_and(|&c| c != b'/') && go(p, s, pi + 1, si + 1, memo),
                Some(&c) => ss.first() == Some(&c) && go(p, s, pi + 1, si + 1, memo),
            }
        };
        memo[slot] = Some(v);
        v
    }
    let mut memo = vec![None; (p.len() + 1) * (s.len() + 1)];
    go(p, s, 0, 0, &mut memo)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    fn manifest(text: &str) -> Manifest {
        Manifest::parse(&parse(text), text)
    }

    #[test]
    fn script_lists_and_sides() {
        let m = manifest(
            "fx_version 'cerulean'\ngame 'gta5'\nlua54 'yes'\nshared_scripts { '@ox_lib/init.lua', 'config.lua' }\nclient_script 'client/*.lua'\nserver_scripts {\n  '@oxmysql/lib/MySQL.lua',\n  'server/**.lua',\n}\ndependency 'ox_lib'\n",
        );
        assert_eq!(m.side("config.lua"), FileSide::Shared);
        assert_eq!(m.side("client/main.lua"), FileSide::Client);
        assert_eq!(m.side("client/sub/x.lua"), FileSide::Unknown);
        assert_eq!(m.side("server/sub/x.lua"), FileSide::Server);
        assert_eq!(m.side("other.lua"), FileSide::Unknown);
        assert!(m.loads_ox_lib(FileSide::Client) && m.loads_ox_lib(FileSide::Server));
        assert_eq!(m.dependencies, ["ox_lib"]);
        assert_eq!(m.game(), Game::Gta5);
        assert!(!m.dynamic);
    }

    #[test]
    fn chained_calls_dynamic_lists_and_rdr() {
        let m = manifest("client_script 'a.lua' 'b.lua'\ngames { 'rdr3' }\n");
        assert_eq!(m.client, ["a.lua", "b.lua"]);
        assert_eq!(m.game(), Game::Rdr3);
        let m = manifest("local files = { 'a.lua' }\nclient_scripts(files)\n");
        assert!(m.dynamic);
        assert_eq!(m.side("a.lua"), FileSide::Unknown);
    }

    #[test]
    fn globs() {
        assert!(glob("client/*.lua", "client/a.lua"));
        assert!(!glob("client/*.lua", "client/x/a.lua"));
        assert!(glob("**/*.lua", "a.lua"));
        assert!(glob("**/*.lua", "x/y/a.lua"));
        assert!(glob("Client/Main.lua", "client/main.lua"));
        assert!(glob("./a.lua", "a.lua"));
        assert!(glob("**/x/**/*.lua", "x/a.lua") && glob("**/x/**/*.lua", "a/x/b/c.lua") && !glob("**/x/**/*.lua", "a/y/c.lua"));
        // Many wildcards over a long path finish quickly.
        let long = "a".repeat(200);
        assert!(!glob(&format!("{}b", "**".repeat(20)), &long));
    }
}
