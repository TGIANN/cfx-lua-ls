//! Document highlight (LuaLS `core/highlight.lua`): a name's occurrences, a
//! keyword's partners in its statement (`if`/`then`/`elseif`/`else`/`end`,
//! `function`/`end`, ...), a literal on its own.

use crate::names::Names;
use crate::{Parse, TextRange, TextSize};
use cfx_lua_syntax::SyntaxKind::{self, *};
use cfx_lua_syntax::{SyntaxNode, SyntaxToken};
use rowan::TokenAtOffset;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighlightKind {
    Text,
    Read,
    Write,
}

pub fn highlights(parse: &Parse, names: &Names, text: &str, offset: TextSize) -> Vec<(TextRange, HighlightKind)> {
    let occurrences = names.highlights(text, offset);
    if !occurrences.is_empty() {
        return occurrences
            .into_iter()
            .map(|(r, write)| (r, if write { HighlightKind::Write } else { HighlightKind::Read }))
            .collect();
    }
    if let Some(sites) = crate::fields::field_references(parse, names, offset) {
        return sites.into_iter().map(|(r, write)| (r, if write { HighlightKind::Write } else { HighlightKind::Read })).collect();
    }
    let Some(token) = token_at(parse, offset) else { return Vec::new() };
    let kind = token.kind();
    if matches!(kind, STRING | LONG_STRING | HASH_STRING | NUMBER | TRUE_KW | FALSE_KW | NIL_KW) {
        return vec![(token.text_range(), HighlightKind::Text)];
    }
    if !kind.is_keyword() {
        return Vec::new();
    }
    keyword_group(&token).into_iter().map(|t| (t.text_range(), HighlightKind::Text)).collect()
}

/// The real token at `offset`, the right one when two touch.
fn token_at(parse: &Parse, offset: TextSize) -> Option<SyntaxToken> {
    let root = parse.syntax();
    if offset > root.text_range().end() {
        return None;
    }
    let pick = |t: &SyntaxToken| !t.kind().is_trivia();
    match root.token_at_offset(offset) {
        TokenAtOffset::None => None,
        TokenAtOffset::Single(t) => pick(&t).then_some(t),
        TokenAtOffset::Between(l, r) => [r, l].into_iter().find(pick),
    }
}

fn direct(node: &SyntaxNode, kinds: &[SyntaxKind]) -> Vec<SyntaxToken> {
    node.children_with_tokens().filter_map(|e| e.into_token()).filter(|t| kinds.contains(&t.kind())).collect()
}

fn keyword_group(token: &SyntaxToken) -> Vec<SyntaxToken> {
    let Some(parent) = token.parent() else { return Vec::new() };
    let func = |owner: &SyntaxNode| {
        let mut out = direct(owner, &[FUNCTION_KW]);
        if let Some(body) = owner.children().find(|c| c.kind() == FUNC_BODY) {
            out.extend(direct(&body, &[END_KW]));
        }
        out
    };
    match parent.kind() {
        LOCAL_FUNCTION_STAT | FUNCTION_STAT | FUNCTION_EXPR if token.kind() == FUNCTION_KW => func(&parent),
        FUNC_BODY => parent.parent().map(|o| func(&o)).unwrap_or_default(),
        IF_STAT | ELSEIF_CLAUSE | ELSE_CLAUSE => {
            let stat = if parent.kind() == IF_STAT { parent } else { parent.parent().unwrap_or(parent) };
            let mut out = direct(&stat, &[IF_KW, THEN_KW, END_KW]);
            for clause in stat.children().filter(|c| matches!(c.kind(), ELSEIF_CLAUSE | ELSE_CLAUSE)) {
                out.extend(direct(&clause, &[ELSEIF_KW, ELSE_KW, THEN_KW]));
            }
            out.sort_by_key(|t| t.text_range().start());
            out
        }
        NUMERIC_FOR_STAT | GENERIC_FOR_STAT => direct(&parent, &[FOR_KW, IN_KW, DO_KW, END_KW]),
        WHILE_STAT => direct(&parent, &[WHILE_KW, DO_KW, END_KW]),
        REPEAT_STAT => direct(&parent, &[REPEAT_KW, UNTIL_KW]),
        DO_STAT => direct(&parent, &[DO_KW, END_KW]),
        _ => vec![token.clone()],
    }
}
