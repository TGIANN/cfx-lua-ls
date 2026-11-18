//! `.editorconfig` as EmmyLuaCodeStyle reads it (`LuaEditorConfig`,
//! `EditorconfigPattern`): sections whose pattern is empty, `*` or `*.lua` always
//! apply, other patterns are matched against the end of the file's URI. The style
//! starts from the defaults, not from `Lua.format.defaultConfig`, as in LuaLS.

use std::collections::HashMap;

use crate::Style;

#[derive(Debug)]
pub struct EditorConfig {
    sections: Vec<Section>,
}

#[derive(Debug)]
struct Section {
    pattern: String,
    matches: Vec<Match>,
    options: HashMap<String, String>,
}

#[derive(Debug)]
enum Match {
    Path(Vec<u8>),
    /// `*`
    AnyExceptSep,
    /// `**`
    Any,
    /// `?`
    AnyChar,
    /// `[abc]`
    CharOf(Vec<u8>),
    /// `[!abc]`
    NotCharOf(Vec<u8>),
    /// `{a,b}`
    StringOf(Vec<Vec<u8>>),
}

fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

fn is_eol(c: u8) -> bool {
    c == b'\n' || c == b'\r'
}

/// `string_util::TrimSpace`.
fn trim(b: &[u8]) -> &[u8] {
    let start = b.iter().position(|&c| !is_space(c)).unwrap_or(b.len());
    let end = b.iter().rposition(|&c| !is_space(c)).map_or(start, |i| i + 1);
    &b[start..end]
}

/// Position after the next line break (`TextReader::NextLine`).
fn next_line(b: &[u8], mut i: usize) -> usize {
    while i < b.len() {
        let c = b[i];
        i += 1;
        if is_eol(c) {
            if c == b'\r' && b.get(i) == Some(&b'\n') {
                i += 1;
            }
            break;
        }
    }
    i
}

impl EditorConfig {
    /// `LuaEditorConfig::Parse`.
    pub fn parse(text: &str) -> EditorConfig {
        let b = text.as_bytes();
        let mut sections = vec![Section::new(b"")];
        let mut i = 0;
        while i < b.len() {
            match b[i] {
                b';' | b'#' | b'\r' | b'\n' => i = next_line(b, i),
                b'\t' | b' ' => {
                    while i < b.len() && is_space(b[i]) {
                        i += 1;
                    }
                }
                b'[' => {
                    let start = i + 1;
                    i = start;
                    while i < b.len() && b[i] != b']' && !is_eol(b[i]) {
                        i += 1;
                    }
                    let pattern = trim(&b[start..i]);
                    let closed = b.get(i) == Some(&b']');
                    i = next_line(b, i);
                    if !pattern.is_empty() && closed {
                        sections.push(Section::new(pattern));
                    }
                }
                _ => {
                    let start = i;
                    while i < b.len() && b[i] != b'=' && !is_eol(b[i]) {
                        i += 1;
                    }
                    let key = trim(&b[start..i]);
                    if key.is_empty() || b.get(i) != Some(&b'=') {
                        i = next_line(b, i);
                        continue;
                    }
                    i += 1;
                    let start = i;
                    while i < b.len() && !is_eol(b[i]) {
                        i += 1;
                    }
                    let value = trim(&b[start..i]);
                    if value.is_empty() {
                        i = next_line(b, i);
                        continue;
                    }
                    let key = String::from_utf8_lossy(key).into_owned();
                    let value = String::from_utf8_lossy(value).into_owned();
                    // `std::map::insert` keeps the first value of a key.
                    sections.last_mut().unwrap().options.entry(key).or_insert(value);
                }
            }
        }
        EditorConfig { sections }
    }

    /// `LuaEditorConfig::Generate` for a file, by its URI as the client sent it.
    pub fn style(&self, file_uri: &str) -> Style {
        let mut style = Style::default();
        for s in &self.sections {
            let always = s.pattern.is_empty() || s.pattern == "*" || s.pattern == "*.lua";
            if (always || s.matches(file_uri)) && !s.options.is_empty() {
                style.apply(&s.options);
            }
        }
        style
    }
}

impl Section {
    fn new(pattern: &[u8]) -> Section {
        Section {
            pattern: String::from_utf8_lossy(pattern).into_owned(),
            matches: compile(pattern),
            options: HashMap::new(),
        }
    }

    /// `EditorconfigPattern::Match`: from the end of the path backwards.
    fn matches(&self, file: &str) -> bool {
        if file.is_empty() {
            return false;
        }
        let path: Vec<u8> = file.bytes().map(|c| if c == b'\\' { b'/' } else { c }).collect();
        let mut view: &[u8] = &path;
        for k in (0..self.matches.len()).rev() {
            let prev_path = match k.checked_sub(1).map(|j| &self.matches[j]) {
                Some(Match::Path(p)) => Some(p),
                _ => None,
            };
            match &self.matches[k] {
                Match::Path(p) => {
                    if !view.ends_with(p) {
                        return false;
                    }
                    view = &view[..view.len() - p.len()];
                }
                Match::AnyExceptSep => {
                    let mut at = view.iter().rposition(|&c| c == b'/' || c == b'\\').map_or(0, |i| i + 1);
                    if let Some(p) = prev_path {
                        let Some(pos) = rfind(view, p) else { return false };
                        if pos + p.len() < at {
                            return false;
                        }
                        at = pos + p.len();
                    }
                    view = &view[..at];
                }
                Match::Any => {
                    let mut at = 0;
                    if let Some(p) = prev_path {
                        let Some(pos) = rfind(view, p) else { return false };
                        at = pos + p.len();
                    }
                    view = &view[..at];
                }
                Match::AnyChar => {
                    let Some((_, rest)) = view.split_last() else { return false };
                    view = rest;
                }
                Match::CharOf(set) => {
                    let Some((last, rest)) = view.split_last() else { return false };
                    if !set.contains(last) {
                        return false;
                    }
                    view = rest;
                }
                Match::NotCharOf(set) => {
                    let Some((last, rest)) = view.split_last() else { return false };
                    if set.contains(last) {
                        return false;
                    }
                    view = rest;
                }
                Match::StringOf(strings) => {
                    let Some(s) = strings.iter().find(|s| view.ends_with(s)) else { return false };
                    view = &view[..view.len() - s.len()];
                }
            }
        }
        true
    }
}

fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.len() > hay.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).rev().find(|&i| hay[i..].starts_with(needle))
}

/// `EditorconfigPattern::Compile` and `Lex`.
fn compile(p: &[u8]) -> Vec<Match> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < p.len() {
        let start = i;
        match p[i] {
            b'*' => {
                i += 1;
                if p.get(i) == Some(&b'*') {
                    i += 1;
                    out.push(Match::Any);
                } else {
                    out.push(Match::AnyExceptSep);
                }
            }
            b'{' => {
                i += 1;
                while i < p.len() {
                    i += 1;
                    if p[i - 1] == b'}' {
                        break;
                    }
                }
                let text = &p[start..i];
                let strings = if text.len() > 2 {
                    text[1..text.len() - 1].split(|&c| c == b',').map(<[u8]>::to_vec).collect()
                } else {
                    Vec::new()
                };
                out.push(Match::StringOf(strings));
            }
            b'[' => {
                i += 1;
                let not = p.get(i) == Some(&b'!');
                if not {
                    i += 1;
                }
                while i < p.len() {
                    i += 1;
                    if p[i - 1] == b']' {
                        break;
                    }
                }
                let text = &p[start..i];
                let skip = if not { 2 } else { 1 };
                let set = if text.len() > skip + 1 { text[skip..text.len() - 1].to_vec() } else { Vec::new() };
                out.push(if not { Match::NotCharOf(set) } else { Match::CharOf(set) });
            }
            b'?' => {
                i += 1;
                out.push(Match::AnyChar);
            }
            _ => {
                i += 1;
                while i < p.len() && !matches!(p[i], b'[' | b'{' | b'*') {
                    i += 1;
                }
                out.push(Match::Path(p[start..i].to_vec()));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::{IndentStyle, QuoteStyle};

    #[test]
    fn sections_and_keys() {
        let c = EditorConfig::parse(
            "# comment\nroot = true\n\n[*.lua]\nindent_style = tab\nindent_size=2\nindent_size = 8\n  quote_style = single  \n\n[{a,b}.lua]\nindent_size = 3\n[broken\nmax_line_length = 50\n",
        );
        let s = c.style("file:///c%3A/x/other.lua");
        assert_eq!((s.indent_style, s.indent_size, s.quote_style), (IndentStyle::Tab, 2, QuoteStyle::Single));
        // `[broken` has no `]`: its keys stay in the previous section.
        assert_eq!(s.max_line_length, 120);
        assert_eq!(c.style("file:///c%3A/x/a.lua").indent_size, 3);
        assert_eq!(c.style("file:///c%3A/x/b.lua").max_line_length, 50);
    }

    #[test]
    fn patterns() {
        let m = |pattern: &str, path: &str| Section::new(pattern.as_bytes()).matches(path);
        assert!(m("test.lua", "file:///c%3A/x/test.lua"));
        assert!(m("src/*.lua", "/w/src/a.lua"));
        assert!(!m("src/*.lua", "/w/src/sub/a.lua"));
        assert!(m("src/**.lua", "/w/src/sub/a.lua"));
        assert!(m("?b.lua", "/w/ab.lua"));
        // As in C++, a `?` after plain text is part of the text.
        assert!(!m("a?.lua", "/w/ab.lua"));
        assert!(m("[ab].lua", "/w/b.lua"));
        assert!(!m("[!ab].lua", "/w/b.lua"));
        assert!(m("{x,y}.lua", "/w/y.lua"));
        assert!(!m("{x,y}.lua", "/w/z.lua"));
        // Backslashes in the path count as slashes.
        assert!(m("dir/f.lua", "C:\\w\\dir\\f.lua"));
    }
}
