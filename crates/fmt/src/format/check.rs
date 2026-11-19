//! EmmyLuaCodeStyle's `CodeStyleChecker`: the same analyzers and walk as the
//! formatter in `FormatState::Mode::Diagnostic`, where "starts a line" is read
//! from the source and nothing is written; each token's resolve is compared
//! with the source instead.

use super::{Fmt, NextSpace, NextTokenStrategy, PrevSpace, Resolve, TokenStrategy, exist_del};
use crate::style::{EndStmtWithSemicolon, IndentStyle};
use crate::tree::Tk;

/// One finding: a byte range of the text and LuaLS's message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleDiagnostic {
    pub start: usize,
    pub len: usize,
    pub message: String,
}

impl Fmt<'_, '_> {
    /// `CodeStyleChecker::Analyze`.
    pub(crate) fn diagnose(mut self) -> Vec<StyleDiagnostic> {
        self.diagnostic = true;
        self.analyze();
        self.walk();
        self.end_with_new_line_check();
        let mut out = std::mem::take(&mut self.diags);
        out.extend(std::mem::take(&mut self.keyed).into_values());
        out
    }

    fn push_diag(&mut self, start: usize, len: usize, message: String) {
        self.diags.push(StyleDiagnostic { start, len, message });
    }

    /// `PushDiagnostic` with a left token: a later push or `ClearDiagnostic` for
    /// the same token replaces it.
    fn push_keyed(&mut self, token: usize, start: usize, len: usize, message: String) {
        self.keyed.insert(token, StyleDiagnostic { start, len, message });
    }

    /// `CodeStyleChecker::BasicResolve`.
    pub(super) fn check(&mut self, n: usize, r: &Resolve) {
        let t = self.t;
        let Some(kind) = t.token_kind(n) else { return };
        let (start, len) = t.text_range(n);
        if self.is_new_line_at(n) {
            self.indent_diagnostic(n);
        }
        if matches!(r.prev_space, PrevSpace::AlignPos(_) | PrevSpace::AlignRelativeIndent(_)) {
            self.keyed.remove(&t.prev_token(n));
        }
        if r.left_paren_before {
            self.push_diag(start, len, "expected (".into());
        }
        match r.token {
            TokenStrategy::StringSingleQuote | TokenStrategy::StringDoubleQuote if kind == Tk::String => {
                let single = r.token == TokenStrategy::StringSingleQuote;
                let (from, del, message) = if single { ('"', '\'', "\" should be ' ") } else { ('\'', '"', "' should be \" ") };
                let text = t.node_text(n);
                if text.len() >= 2 && text.starts_with(from) && !exist_del(del, text) {
                    self.push_diag(start, len, message.into());
                }
            }
            TokenStrategy::StmtEndSemicolon => match self.style.end_statement_with_semicolon {
                EndStmtWithSemicolon::ReplaceWithNewline => self.push_diag(start, len, "expected statement not to end with ;".into()),
                EndStmtWithSemicolon::SameLine => {
                    self.push_diag(start, len, "; should only separate multiple statements on a single line".into())
                }
                _ => {}
            },
            _ => {}
        }
        match r.next_token {
            NextTokenStrategy::StmtEndSemicolon => self.push_diag(start + len - 1, 1, "expected ; at end of statement".into()),
            NextTokenStrategy::RightParentheses => self.push_diag(start, len, "expected )".into()),
            _ => {}
        }
        if let NextSpace::Space(space) = r.next_space {
            let next = t.next_token(n);
            if t.is_token(next) && t.start_line(next) == t.end_line(n) {
                self.space_diagnostic(n, next, space);
            }
        }
    }

    /// `ProcessSpaceDiagnostic`; offsets as C++'s inclusive end offset.
    fn space_diagnostic(&mut self, n: usize, next: usize, should: usize) {
        let t = self.t;
        let (start, len) = t.text_range(n);
        let left = start + len - 1;
        let right = t.text_range(next).0;
        if right < left + 1 {
            return;
        }
        let diff = right - left - 1;
        if diff == should {
            return;
        }
        let note = self.additional_note(n, next);
        let (at, len, message) = match should {
            0 => (left + 1, diff, format!("unnecessary whitespace {note}")),
            1 if diff == 0 => (left, 2, format!("missing whitespace {note}")),
            1 => (left + 1, diff, format!("multiple spaces {note}")),
            _ if diff < should => (left, diff + 1, format!("expected {should} whitespace, found {diff} {note}")),
            _ => (left + 1, diff, format!("expected {should} whitespace, found {diff} {note}")),
        };
        self.push_keyed(n, at, len, message);
    }

    /// `GetAdditionalNote`.
    fn additional_note(&self, left: usize, right: usize) -> String {
        let t = self.t;
        let plain = |n: usize| matches!(t.token_kind(n), Some(Tk::String | Tk::LongString | Tk::LongComment | Tk::ShortComment | Tk::Name));
        if !plain(left) {
            return format!("after token '{}'", t.node_text(left));
        }
        if plain(right) { String::new() } else { format!("before token '{}'", t.node_text(right)) }
    }

    /// `ProcessIndentDiagnostic`: the line's leading tabs then spaces against the
    /// indentation the walk is at.
    fn indent_diagnostic(&mut self, n: usize) {
        let t = self.t;
        self.indent_mark(n);
        let indent = self.current_indent();
        let start = t.text_range(n).0;
        let line_start = t.line_start(start);
        let bytes = t.text().as_bytes();
        let mut end = line_start;
        while end < start && matches!(bytes[end], b'\t' | b' ') {
            end += 1;
        }
        let (mut tabs, mut spaces) = (0, 0);
        for &b in &bytes[line_start..end] {
            if b == b'\t' && spaces == 0 {
                tabs += 1;
            } else {
                spaces += 1;
            }
        }
        if indent.tab != tabs || indent.space != spaces {
            let message = if self.style.indent_style == IndentStyle::Tab {
                format!("expected {} tab indent, found {tabs} tab", indent.tab)
            } else {
                format!("expected {} whitespace indent, found {spaces} whitespace", indent.space)
            };
            self.push_diag(line_start, end - line_start, message);
        }
    }

    /// `CodeStyleChecker::EndWithNewLine`.
    fn end_with_new_line_check(&mut self) {
        let text = self.t.text();
        if text.is_empty() {
            return;
        }
        let ends = text.ends_with(['\n', '\r']);
        if self.style.insert_final_newline && !ends {
            self.push_diag(text.len(), 0, "must end with new line".into());
        } else if !self.style.insert_final_newline && ends {
            self.push_diag(text.len(), 0, "can not end with new line".into());
        }
    }
}
