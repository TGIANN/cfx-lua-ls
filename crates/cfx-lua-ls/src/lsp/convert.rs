//! Offsets (`TextSize`, UTF-8) to LSP positions and back, in the encoding agreed
//! at `initialize`. Positions from the client are clamped: a line past the end
//! maps to the end of the text, a column past the line to the end of the line.

use cfx_lua_ide::{TextRange, TextSize};
use line_index::{LineCol, LineIndex, WideEncoding, WideLineCol};
use lsp_types::{Position, Range};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Encoding {
    Utf8,
    Utf16,
}

pub(crate) fn position(index: &LineIndex, offset: TextSize, enc: Encoding) -> Position {
    let lc = index.line_col(offset.min(index.len()));
    match enc {
        Encoding::Utf8 => Position::new(lc.line, lc.col),
        Encoding::Utf16 => {
            let wide = index.to_wide(WideEncoding::Utf16, lc).unwrap_or(WideLineCol { line: lc.line, col: lc.col });
            Position::new(wide.line, wide.col)
        }
    }
}

pub(crate) fn range(index: &LineIndex, range: TextRange, enc: Encoding) -> Range {
    Range::new(position(index, range.start(), enc), position(index, range.end(), enc))
}

pub(crate) fn offset(index: &LineIndex, text: &str, pos: Position, enc: Encoding) -> TextSize {
    let Some(line) = index.line(pos.line) else { return index.len() };
    // The line's content, without its line break.
    let line_text = &text[line];
    let content_len = line_text.trim_end_matches(['\n', '\r']).len() as u32;
    let col = match enc {
        Encoding::Utf8 => pos.character,
        Encoding::Utf16 => index
            .to_utf8(WideEncoding::Utf16, WideLineCol { line: pos.line, col: pos.character })
            .map_or(u32::MAX, |lc: LineCol| lc.col),
    };
    let mut col = col.min(content_len) as usize;
    // A position inside a character (half of a surrogate pair) snaps back to its start.
    while !line_text.is_char_boundary(col) {
        col -= 1;
    }
    line.start() + TextSize::from(col as u32)
}

pub(crate) fn text_range(index: &LineIndex, text: &str, range: Range, enc: Encoding) -> TextRange {
    let start = offset(index, text, range.start, enc);
    let end = offset(index, text, range.end, enc);
    TextRange::new(start.min(end), start.max(end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_round_trip_and_clamping() {
        let text = "a😀b\r\nçx\n";
        let index = LineIndex::new(text);
        // 😀 is 4 UTF-8 bytes and 2 UTF-16 units.
        let b = TextSize::from(5);
        assert_eq!(position(&index, b, Encoding::Utf16), Position::new(0, 3));
        assert_eq!(offset(&index, text, Position::new(0, 3), Encoding::Utf16), b);
        // Inside the surrogate pair: snaps to the emoji's start.
        assert_eq!(offset(&index, text, Position::new(0, 2), Encoding::Utf16), TextSize::from(1));
        // Past the line: end of the content, before "\r\n".
        assert_eq!(offset(&index, text, Position::new(0, 99), Encoding::Utf16), TextSize::from(6));
        // Past the last line: end of the text.
        assert_eq!(offset(&index, text, Position::new(9, 0), Encoding::Utf16), TextSize::of(text));
        // UTF-8 columns are bytes.
        assert_eq!(offset(&index, text, Position::new(1, 2), Encoding::Utf8), TextSize::from(10));
    }
}
