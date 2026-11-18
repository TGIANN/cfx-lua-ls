//! An open document: its text, version and tree. Escrow files keep no tree.

use super::convert::{self, Encoding};
use cfx_lua_ide::{Names, Parse, is_escrow, parse};
use std::cell::OnceCell;
use line_index::LineIndex;
use lsp_types::TextDocumentContentChangeEvent;

pub(crate) struct Document {
    pub(crate) text: String,
    pub(crate) version: i32,
    pub(crate) index: LineIndex,
    /// `None` for an escrow file. Only `Parse` is kept: it frees deep trees safely,
    /// a stored `SyntaxNode` would not.
    pub(crate) parse: Option<Parse>,
    /// Name resolution, made on the first request that needs it.
    names: OnceCell<Names>,
}

impl Document {
    pub(crate) fn new(text: String, version: i32) -> Self {
        let index = LineIndex::new(&text);
        let parse = (!is_escrow(&text)).then(|| parse(&text));
        Document { text, version, index, parse, names: OnceCell::new() }
    }

    /// `None` for an escrow file.
    pub(crate) fn names(&self) -> Option<&Names> {
        let parse = self.parse.as_ref()?;
        Some(self.names.get_or_init(|| cfx_lua_ide::names(parse, &self.text)))
    }

    /// Applies the changes in order, each to the text the previous one produced,
    /// then parses again.
    pub(crate) fn change(&mut self, changes: Vec<TextDocumentContentChangeEvent>, version: i32, enc: Encoding) {
        for change in changes {
            match change.range {
                None => self.text = change.text,
                Some(range) => {
                    let index = LineIndex::new(&self.text);
                    let r = convert::text_range(&index, &self.text, range, enc);
                    self.text.replace_range(std::ops::Range::<usize>::from(r), &change.text);
                }
            }
        }
        *self = Document::new(std::mem::take(&mut self.text), version);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{Position, Range};

    fn edit(sl: u32, sc: u32, el: u32, ec: u32, text: &str) -> TextDocumentContentChangeEvent {
        TextDocumentContentChangeEvent {
            range: Some(Range::new(Position::new(sl, sc), Position::new(el, ec))),
            range_length: None,
            text: text.into(),
        }
    }

    #[test]
    fn batched_edits_apply_in_order() {
        let mut doc = Document::new("local a😀 = 1\r\nprint(a)\r\n".into(), 1);
        doc.change(
            vec![
                edit(0, 6, 0, 9, "b"),     // "a😀" -> "b" (UTF-16: a + 2 units)
                edit(1, 6, 1, 7, "b"),     // on the text the first edit produced
                edit(5, 0, 5, 0, "-- x"), // past the end: appended
            ],
            2,
            Encoding::Utf16,
        );
        assert_eq!(doc.text, "local b = 1\r\nprint(b)\r\n-- x");
        assert_eq!(doc.version, 2);
        assert!(doc.parse.as_ref().is_some_and(|p| p.errors().is_empty()));
    }

    #[test]
    fn escrow_files_get_no_tree() {
        let doc = Document::new("FXAP\u{1}\u{2}garbage".into(), 1);
        assert!(doc.parse.is_none());
    }
}
