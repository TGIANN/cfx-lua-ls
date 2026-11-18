//! Selection ranges (expand / shrink selection). LuaLS has no such feature; this
//! follows the syntax tree: the token under the cursor (a string's contents first),
//! then every enclosing node from its first to its last real token, up to the file.
//! Each step strictly contains the one before.

use crate::util::trimmed;
use crate::{Parse, TextRange, TextSize};
use cfx_lua_syntax::SyntaxKind::{self, *};
use cfx_lua_syntax::{SyntaxNode, SyntaxToken};
use rowan::{NodeOrToken, TokenAtOffset};

/// Longest chain returned. A 20,000-link `a.b.b...` has as many nodes; past this
/// the innermost and outermost halves are kept, the middle steps are dropped.
pub const MAX_SELECTION_STEPS: usize = 200;

/// Innermost range first; empty when the file is empty.
pub fn selection_ranges(parse: &Parse, offset: TextSize) -> Vec<TextRange> {
    let root = parse.syntax();
    let offset = offset.min(root.text_range().end());
    let mut steps: Vec<TextRange> = Vec::new();
    let push = |steps: &mut Vec<TextRange>, r: TextRange| {
        let r = steps.last().map_or(r, |last| r.cover(*last));
        if steps.last() != Some(&r) {
            steps.push(r);
        }
    };
    let Some(token) = pick_token(token_at(&root, offset)) else { return steps };
    if token.kind() != WHITESPACE {
        if let Some(inner) = string_contents(&token)
            && inner.contains_inclusive(offset)
            && !inner.is_empty()
        {
            push(&mut steps, inner);
        }
        push(&mut steps, token.text_range());
    }
    let mut node = token.parent();
    while let Some(n) = node {
        if let Some(r) = trimmed(&n) {
            push(&mut steps, r);
        }
        node = n.parent();
    }
    push(&mut steps, root.text_range());
    if steps.len() > MAX_SELECTION_STEPS {
        let half = MAX_SELECTION_STEPS / 2;
        steps.drain(half..steps.len() - half);
    }
    steps
}

/// rowan's `token_at_offset` recurses once per tree level and overflows the stack on
/// a deep chain; this descends with one binary search per level.
pub(crate) fn token_at(root: &SyntaxNode, offset: TextSize) -> TokenAtOffset<SyntaxToken> {
    let covering = |at: TextSize| {
        let mut node = root.clone();
        loop {
            match node.child_or_token_at_range(TextRange::at(at, TextSize::from(1)))? {
                NodeOrToken::Node(n) => node = n,
                NodeOrToken::Token(t) => return Some(t),
            }
        }
    };
    let right = (offset < root.text_range().end()).then(|| covering(offset)).flatten();
    let left = (offset > TextSize::from(0)).then(|| covering(offset - TextSize::from(1))).flatten();
    match (left, right) {
        (Some(l), Some(r)) if l == r => TokenAtOffset::Single(r),
        (Some(l), Some(r)) => TokenAtOffset::Between(l, r),
        (Some(t), None) | (None, Some(t)) => TokenAtOffset::Single(t),
        (None, None) => TokenAtOffset::None,
    }
}

/// Between two tokens, the one that reads as a word wins (a name, keyword or
/// literal), then punctuation, then trivia; on a tie the right one.
fn pick_token(at: TokenAtOffset<SyntaxToken>) -> Option<SyntaxToken> {
    let rank = |k: SyntaxKind| {
        if k.is_trivia() {
            0
        } else if matches!(k, NAME | NUMBER | STRING | LONG_STRING | HASH_STRING) || k.is_keyword() {
            2
        } else {
            1
        }
    };
    match at {
        TokenAtOffset::None => None,
        TokenAtOffset::Single(t) => Some(t),
        TokenAtOffset::Between(l, r) => Some(if rank(l.kind()) > rank(r.kind()) { l } else { r }),
    }
}

/// A string without its quotes or long brackets. An unterminated string keeps its
/// end: cutting a closing delimiter that is not there would end inside a character.
fn string_contents(token: &SyntaxToken) -> Option<TextRange> {
    let text = token.text();
    let (open, closer) = match token.kind() {
        STRING | HASH_STRING => (1, text.get(..1)?.to_string()),
        LONG_STRING => {
            let level = text.get(1..)?.bytes().take_while(|&b| b == b'=').count();
            (level + 2, format!("]{}]", "=".repeat(level)))
        }
        _ => return None,
    };
    let body = text.get(open..)?;
    // A quote counts as closing unless an odd run of backslashes escapes it.
    let closed = body.ends_with(closer.as_str()) && {
        let before = &body[..body.len() - closer.len()];
        token.kind() == LONG_STRING || before.bytes().rev().take_while(|&b| b == b'\\').count() % 2 == 0
    };
    let close = if closed { closer.len() } else { 0 };
    let start = token.text_range().start() + TextSize::from(open as u32);
    let end = token.text_range().end() - TextSize::from(close as u32);
    (start <= end).then(|| TextRange::new(start, end))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    /// The selected texts, innermost first, for the cursor at `|`.
    fn steps(marked: &str) -> Vec<String> {
        let at = marked.find('|').unwrap();
        let text = marked.replace('|', "");
        let parse = parse(&text);
        selection_ranges(&parse, TextSize::from(at as u32)).iter().map(|&r| text[r].to_string()).collect()
    }

    #[test]
    fn grows_from_the_token_to_the_file() {
        assert_eq!(
            steps("local x = 1\nlocal y = a.b|c + 2\n"),
            ["bc", "a.bc", "a.bc + 2", "local y = a.bc + 2", "local x = 1\nlocal y = a.bc + 2", "local x = 1\nlocal y = a.bc + 2\n"]
        );
    }

    #[test]
    fn string_contents_come_first() {
        assert_eq!(steps("print('he|llo')")[..3], ["hello", "'hello'", "('hello')"]);
        assert_eq!(steps("x = [==[lo|ng]==]")[..2], ["long", "[==[long]==]"]);
        // Unterminated: no closing delimiter to cut.
        assert_eq!(steps("x = 'a|ç")[0], "aç");
        assert_eq!(steps(r"x = 'a|\'")[0], r"a\'");
        assert_eq!(steps("x = [[a|ç")[0], "aç");
    }

    #[test]
    fn a_comment_above_a_statement_stays_inside_the_steps() {
        let s = steps("-- no|te\nlocal x = 1\n");
        assert_eq!(s[0], "-- note");
        assert_eq!(s[1], "-- note\nlocal x = 1");
    }

    #[test]
    fn between_tokens_the_word_wins() {
        assert_eq!(steps("f(a|)")[0], "a");
        assert_eq!(steps("f(|a)")[0], "a");
    }

    #[test]
    fn deep_chains_are_capped() {
        let text = format!("x = a{}", ".b".repeat(5_000));
        let parse = parse(&text);
        let s = selection_ranges(&parse, TextSize::from(5));
        assert_eq!(s.len(), MAX_SELECTION_STEPS);
        assert!(s.windows(2).all(|w| w[1].contains_range(w[0]) && w[1] != w[0]));
        assert_eq!(*s.last().unwrap(), parse.syntax().text_range());
    }
}

