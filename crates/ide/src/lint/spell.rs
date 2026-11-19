//! `spell-check`, off by default as in LuaLS: EmmyLuaCodeStyle's
//! `CodeSpellChecker` over names and short strings, with LuaLS's two
//! dictionaries (`data/spell`) and `Lua.spell.dict`.
//!
//! Suggestions (`spell_suggest`) are SymSpell's `LookUp` with its settings
//! (edit distance 2, closest matches only, most frequent first), found by
//! comparing every dictionary word instead of through SymSpell's delete index;
//! the result is the same set.

use super::Linter;
use crate::TextRange;
use cfx_lua_syntax::SyntaxKind::*;
use std::collections::HashMap;
use std::sync::OnceLock;

const DICTIONARY: &str = include_str!("../../data/spell/dictionary.txt");
const LUA_DICTIONARY: &str = include_str!("../../data/spell/lua_dict.txt");

/// Word -> count, as `SymSpell::LoadWordDictionaryFromBuffer` builds it: a line
/// is `word [count]`, a repeated word adds its count.
fn dictionary() -> &'static HashMap<&'static str, u64> {
    static DICT: OnceLock<HashMap<&'static str, u64>> = OnceLock::new();
    DICT.get_or_init(|| {
        let mut words: HashMap<&'static str, u64> = HashMap::with_capacity(90_000);
        for line in DICTIONARY.lines().chain(LUA_DICTIONARY.lines()) {
            let mut parts = line.split(' ').filter(|p| !p.is_empty());
            let Some(word) = parts.next() else { continue };
            let count = match (parts.next(), parts.next()) {
                (Some(c), None) => c.parse::<u64>().unwrap_or(0).min(i32::MAX as u64),
                _ => 1,
            };
            if count > 0 {
                let e = words.entry(word).or_insert(0);
                *e = (*e + count).min(i32::MAX as u64);
            }
        }
        words
    })
}

/// `SymSpell::IsCorrectWord` (edit distance 2).
fn correct(word: &str) -> bool {
    word.len() <= 2 || dictionary().contains_key(word)
}

/// `spell::identify::ParseToWords`: the lower-case runs longer than three
/// letters of a name (`getPlayerName` -> `Player`, `Name`), as byte ranges. A
/// character that is not ASCII alphanumeric or `_` drops the whole name.
fn words(s: &str) -> Vec<(usize, usize)> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'_' {
            i += 1;
            continue;
        }
        if !c.is_ascii_alphanumeric() {
            return Vec::new();
        }
        let start = i;
        i += 1;
        let eat = |i: &mut usize, f: fn(&u8) -> bool| {
            let from = *i;
            while *i < b.len() && f(&b[*i]) {
                *i += 1;
            }
            *i - from
        };
        let ascii = if c.is_ascii_lowercase() {
            eat(&mut i, u8::is_ascii_lowercase);
            true
        } else if c.is_ascii_digit() {
            eat(&mut i, u8::is_ascii_digit);
            false
        } else {
            let upper = eat(&mut i, u8::is_ascii_uppercase);
            eat(&mut i, u8::is_ascii_lowercase);
            upper == 0
        };
        if ascii && i - start > 3 {
            out.push((start, i - start));
        }
    }
    out
}

/// `spell::text::ParseToIdentifies`: the space-separated runs longer than three
/// bytes of a quoted string, as byte ranges in the token (a `\` skips the next
/// byte between runs).
fn identifiers(token: &str) -> Vec<(usize, usize)> {
    let b = token.as_bytes();
    if b.len() < 3 || !matches!(b[0], b'\'' | b'"') {
        return Vec::new();
    }
    let b = &b[1..b.len() - 1];
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'\\' {
            i += 2;
            continue;
        }
        if is_space(c) {
            i += 1;
            continue;
        }
        let start = i;
        while i < b.len() && !is_space(b[i]) {
            i += 1;
        }
        if i - start > 3 {
            out.push((start + 1, i - start));
        }
    }
    out
}

/// C's `isspace`.
fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

pub(super) fn run(l: &mut Linter) {
    if !l.enabled("spell-check") {
        return;
    }
    let custom = &l.ctx.config.spell_dict;
    let known = |w: &str| {
        let lower = w.to_ascii_lowercase();
        custom.contains(&lower) || correct(&lower)
    };
    let mut found = Vec::new();
    for t in l.root.descendants_with_tokens().filter_map(|e| e.into_token()) {
        let start = usize::from(t.text_range().start());
        let text = t.text();
        match t.kind() {
            NAME => {
                if custom.iter().any(|c| c == text) {
                    continue;
                }
                for (at, len) in words(text) {
                    if !known(&text[at..at + len]) {
                        found.push((start + at, len, format!("Typo in identifier '{}'", &text[at..at + len])));
                    }
                }
            }
            STRING => {
                for (id_at, id_len) in identifiers(text) {
                    let id = &text[id_at..id_at + id_len];
                    for (at, len) in words(id) {
                        if !known(&id[at..at + len]) {
                            found.push((start + id_at + at, len, format!("Typo in string '{}'", &id[at..at + len])));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    for (start, len, message) in found {
        l.push("spell-check", TextRange::at((start as u32).into(), (len as u32).into()), message);
    }
}

/// Optimal string alignment distance (SymSpell's `DamerauOSA`).
fn osa(a: &[u8], b: &[u8]) -> usize {
    let (n, m) = (a.len(), b.len());
    let mut d = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut v = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                v = v.min(d[i - 2][j - 2] + 1);
            }
            d[i][j] = v;
        }
    }
    d[n][m]
}

/// `LuaCodeFormat::SpellCorrect` as LuaLS's spell code action asks for it: at
/// most 14 words for a misspelt one, with its first letter's case.
pub fn spell_suggest(word: &str) -> Vec<String> {
    let lower = word.to_ascii_lowercase();
    if lower.is_empty() || !lower.bytes().all(|c| c.is_ascii_alphabetic()) || dictionary().contains_key(lower.as_str()) {
        return Vec::new();
    }
    let input = lower.as_bytes();
    let mut best = 3;
    let mut found: Vec<(&str, u64)> = Vec::new();
    for (&w, &count) in dictionary() {
        if w.len().abs_diff(input.len()) > 2 {
            continue;
        }
        let d = osa(input, w.as_bytes());
        if d < best {
            best = d;
            found.clear();
        }
        if d == best {
            found.push((w, count));
        }
    }
    found.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let upper = word.as_bytes()[0].is_ascii_uppercase();
    found
        .into_iter()
        .take(14)
        .map(|(w, _)| {
            let mut s = w.to_string();
            if upper {
                s[..1].make_ascii_uppercase();
            }
            s
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits() {
        let w = |s: &str| words(s).into_iter().map(|(a, n)| s[a..a + n].to_string()).collect::<Vec<_>>();
        assert_eq!(w("getPlayerName"), ["Player", "Name"]);
        assert_eq!(w("HTTPServer_value"), ["value"]);
        assert_eq!(w("abc_defg12hijk"), ["defg", "hijk"]);
        assert!(w("çalış").is_empty());
        assert_eq!(identifiers("'hello wrld\\n x'"), [(1, 5), (7, 6)]);
    }

    #[test]
    fn dictionary_and_suggestions() {
        assert!(correct("player") && correct("setmetatable") && !correct("wrld"));
        assert_eq!(spell_suggest("Wrld").first().map(String::as_str), Some("World"));
        assert!(spell_suggest("world").is_empty());
    }
}
