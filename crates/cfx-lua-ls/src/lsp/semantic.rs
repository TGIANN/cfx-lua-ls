//! Semantic tokens in the LSP's relative encoding. A token over several lines (long
//! string, long comment) is cut into one piece per line: not every client supports
//! multi-line tokens, and one piece per line is valid for all of them.

use super::convert::{self, Encoding};
use cfx_lua_ide::{HighlightToken, TOKEN_MODIFIERS, TOKEN_TYPES, TextRange, TextSize};
use line_index::LineIndex;
use lsp_types::{SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokensLegend};

pub(crate) fn legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: TOKEN_TYPES.iter().map(|&t| SemanticTokenType::new(t)).collect(),
        token_modifiers: TOKEN_MODIFIERS.iter().map(|&m| SemanticTokenModifier::new(m)).collect(),
    }
}

pub(crate) fn encode(index: &LineIndex, text: &str, tokens: &[HighlightToken], enc: Encoding) -> Vec<SemanticToken> {
    let mut out = Vec::with_capacity(tokens.len());
    let (mut prev_line, mut prev_char) = (0, 0);
    for t in tokens {
        for piece in line_pieces(index, text, t.range) {
            let start = convert::position(index, piece.start(), enc);
            let end = convert::position(index, piece.end(), enc);
            let delta_line = start.line - prev_line;
            let delta_start = if delta_line == 0 { start.character - prev_char } else { start.character };
            out.push(SemanticToken {
                delta_line,
                delta_start,
                length: end.character - start.character,
                token_type: t.ty as u32,
                token_modifiers_bitset: t.modifiers,
            });
            (prev_line, prev_char) = (start.line, start.character);
        }
    }
    out
}

/// `range` cut at line breaks, without the breaks and without empty pieces.
fn line_pieces(index: &LineIndex, text: &str, range: TextRange) -> Vec<TextRange> {
    let first = index.line_col(range.start()).line;
    let last = index.line_col(range.end()).line;
    if first == last {
        return vec![range];
    }
    let mut pieces = Vec::new();
    for line in first..=last {
        let Some(span) = index.line(line) else { break };
        let content_end = span.start() + TextSize::of(text[span].trim_end_matches(['\n', '\r']));
        let start = span.start().max(range.start());
        let end = content_end.min(range.end());
        if start < end {
            pieces.push(TextRange::new(start, end));
        }
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multi_line_tokens_are_cut_per_line() {
        let text = "x = [[ab\r\nçd\r\n]] y";
        let index = LineIndex::new(text);
        let start = TextSize::from(4);
        let end = TextSize::from(text.find("]]").unwrap() as u32 + 2);
        let token = HighlightToken { range: TextRange::new(start, end), ty: cfx_lua_ide::TokenType::String, modifiers: 0 };
        let got: Vec<_> =
            encode(&index, text, &[token], Encoding::Utf16).iter().map(|t| (t.delta_line, t.delta_start, t.length)).collect();
        // `[[ab` at 4, `çd` (2 UTF-16 units) on the next line, `]]` on the last.
        assert_eq!(got, [(0, 4, 4), (1, 0, 2), (1, 0, 2)]);
    }
}
