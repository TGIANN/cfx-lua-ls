//! `TokenAnalyzer` (quotes, table separators, call parentheses, comment dashes),
//! `SemicolonAnalyzer` and `FormatDocAnalyze` (`---@format disable...`).

use super::space::Priority;
use super::{Fmt, NextTokenStrategy, Resolve, TokenStrategy};
use crate::style::{CallArgParentheses, EndStmtWithSemicolon, QuoteStyle, TableSeparatorStyle, TrailingTableSeparator};
use crate::tree::{Nk, Tk};

#[derive(Debug, Clone, Copy)]
enum Semicolon {
    Add,
    Remove,
    InsertNewLine,
}

pub(crate) struct State {
    token: Vec<Option<TokenStrategy>>,
    next: Vec<Option<NextTokenStrategy>>,
    left_paren: Vec<bool>,
    semicolon: Vec<Option<Semicolon>>,
    pub(super) ignores: Vec<Option<(usize, usize)>>,
}

impl State {
    pub(super) fn new(n: usize) -> State {
        State {
            token: vec![None; n],
            next: vec![None; n],
            left_paren: vec![false; n],
            semicolon: vec![None; n],
            ignores: vec![None; n],
        }
    }
}

impl Fmt<'_, '_> {
    pub(super) fn token_analyze(&mut self) {
        let t = self.t;
        let s = self.style;
        for n in t.all() {
            match t.node_kind(n) {
                Some(Nk::TableField) => self.token_table_field(n),
                Some(Nk::CallExpression) => {
                    if s.call_arg_parentheses != CallArgParentheses::Keep {
                        self.token_call_expression(n);
                    }
                    if s.remove_call_expression_list_finish_comma {
                        let last = t.last_child(t.child_node(n, Nk::ExpressionList));
                        if t.token_kind(last) == Some(Tk::Comma) {
                            self.mark_token(last, TokenStrategy::Remove);
                        }
                    }
                }
                Some(_) => {}
                None => match t.token_kind(n) {
                    Some(Tk::String) => match s.quote_style {
                        QuoteStyle::Single => self.mark_token(n, TokenStrategy::StringSingleQuote),
                        QuoteStyle::Double => self.mark_token(n, TokenStrategy::StringDoubleQuote),
                        QuoteStyle::None => {}
                    },
                    Some(Tk::ShortComment) if s.space_after_comment_dash => {
                        let text = t.node_text(n).as_bytes();
                        let dashes = text.iter().take_while(|&&b| b == b'-').count();
                        if (dashes == 2 || dashes == 3) && dashes < text.len() && text[dashes] != b' ' {
                            self.mark_token(n, TokenStrategy::SpaceAfterCommentDash);
                        }
                    }
                    _ => {}
                },
            }
        }
    }

    pub(super) fn token_query(&mut self, n: usize, r: &mut Resolve) {
        if !self.t.is_token(n) {
            return;
        }
        if self.token.left_paren[n] {
            r.left_paren_before = true;
        }
        if let Some(s) = self.token.token[n] {
            r.token = s;
        }
        if let Some(s) = self.token.next[n] {
            r.next_token = s;
        }
    }

    pub(super) fn token_is_remove(&self, n: usize) -> bool {
        self.token.token[n] == Some(TokenStrategy::Remove)
    }

    fn mark_token(&mut self, n: usize, s: TokenStrategy) {
        self.token.token[n] = Some(s);
    }

    fn mark_next(&mut self, n: usize, s: NextTokenStrategy) {
        self.token.next[n] = Some(s);
    }

    fn table_field_add_sep(&mut self, n: usize) {
        let t = self.t;
        if t.is_node(t.child_node(n, Nk::TableFieldSep)) {
            return;
        }
        let mut last = t.last_token(n);
        if matches!(t.token_kind(last), Some(Tk::ShortComment | Tk::LongComment)) {
            last = t.prev_token(last);
        }
        let colon = match self.style.table_separator_style {
            TableSeparatorStyle::Semicolon => true,
            TableSeparatorStyle::OnlyKvColon => t.is_token(t.child_token(n, Tk::Assign)),
            _ => false,
        };
        let s = if colon { NextTokenStrategy::TableAddColon } else { NextTokenStrategy::TableAddComma };
        self.mark_next(last, s);
    }

    fn token_table_field(&mut self, n: usize) {
        let t = self.t;
        let sep = t.child_node(n, Nk::TableFieldSep);
        let (from, to) = match self.style.table_separator_style {
            TableSeparatorStyle::Semicolon => (Some(Tk::Comma), TokenStrategy::TableSepSemicolon),
            TableSeparatorStyle::Comma => (Some(Tk::Semi), TokenStrategy::TableSepComma),
            TableSeparatorStyle::OnlyKvColon if t.is_token(t.child_token(n, Tk::Assign)) => {
                (Some(Tk::Comma), TokenStrategy::TableSepSemicolon)
            }
            TableSeparatorStyle::OnlyKvColon => (Some(Tk::Semi), TokenStrategy::TableSepComma),
            TableSeparatorStyle::None => (None, TokenStrategy::Origin),
        };
        if let Some(from) = from {
            let token = t.child_token(sep, from);
            if t.is_token(token) {
                self.mark_token(token, to);
            }
        }

        let trailing = self.style.trailing_table_separator;
        if trailing == TrailingTableSeparator::Keep {
            return;
        }
        let next = t.next_token_skip_comment(n);
        if t.token_kind(next) != Some(Tk::RCurly) {
            return;
        }
        let remove_sep = |f: &mut Self| {
            let first = t.first_token(sep);
            f.mark_token(first, TokenStrategy::Remove);
        };
        match trailing {
            TrailingTableSeparator::Never => remove_sep(self),
            TrailingTableSeparator::Always => self.table_field_add_sep(n),
            TrailingTableSeparator::Smart => {
                if t.end_line(t.parent(n)) == t.start_line(next) {
                    remove_sep(self);
                } else {
                    self.table_field_add_sep(n);
                }
            }
            TrailingTableSeparator::Keep => {}
        }
    }

    /// `GetSingleArgStringOrTable`; `None` when the call has no single table or
    /// string argument (`IsSingleTableOrStringArg` is false).
    fn single_string_or_table_arg(&self, n: usize) -> Option<usize> {
        let t = self.t;
        for c in t.children(n) {
            match t.node_kind(c) {
                Some(Nk::StringLiteralExpression | Nk::TableExpression) => return Some(c),
                Some(Nk::ExpressionList) => {
                    if let [expr] = t.child_exprs(c)[..] {
                        match t.node_kind(expr) {
                            Some(Nk::TableExpression) => return Some(expr),
                            Some(Nk::StringLiteralExpression) => {
                                let first = t.first_token(expr);
                                let string = matches!(t.token_kind(first), Some(Tk::String | Tk::LongString));
                                return Some(if string { first } else { 0 });
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        None
    }

    fn token_call_expression(&mut self, n: usize) {
        let t = self.t;
        let Some(arg) = self.single_string_or_table_arg(n) else { return };
        let lparen = t.child_token(n, Tk::LParen);
        let remove_parens = |f: &mut Self| {
            if t.is_token(lparen) {
                f.mark_token(lparen, TokenStrategy::Remove);
                f.mark_token(t.child_token(n, Tk::RParen), TokenStrategy::Remove);
            }
        };
        match self.style.call_arg_parentheses {
            CallArgParentheses::Remove => remove_parens(self),
            CallArgParentheses::RemoveStringOnly => {
                if matches!(t.token_kind(arg), Some(Tk::String | Tk::LongString)) {
                    remove_parens(self);
                }
            }
            CallArgParentheses::RemoveTableOnly => {
                if t.node_kind(arg) == Some(Nk::TableExpression) {
                    remove_parens(self);
                }
            }
            CallArgParentheses::Always if !t.is_token(lparen) => {
                if t.is_token(arg) {
                    self.token.left_paren[arg] = true;
                    self.mark_next(arg, NextTokenStrategy::RightParentheses);
                    self.space_around_p(arg, 0, Priority::First);
                } else if t.node_kind(arg) == Some(Nk::StringLiteralExpression) {
                    let first = t.first_token(arg);
                    self.token.left_paren[first] = true;
                    self.mark_next(first, NextTokenStrategy::RightParentheses);
                    self.space_left_p(first, 0, Priority::First);
                } else {
                    let first = t.first_token(arg);
                    self.token.left_paren[first] = true;
                    self.mark_next(t.last_token(arg), NextTokenStrategy::RightParentheses);
                    self.space_left_p(first, 0, Priority::First);
                }
                return;
            }
            CallArgParentheses::Always | CallArgParentheses::Keep => {}
        }
        self.function_call_single_arg_space(n);
    }

    pub(super) fn semicolon_analyze(&mut self) {
        let t = self.t;
        let style = self.style.end_statement_with_semicolon;
        if style == EndStmtWithSemicolon::Keep {
            return;
        }
        for n in t.all() {
            let Some(kind) = t.node_kind(n) else { continue };
            if !kind.is_stmt() {
                continue;
            }
            let last = self.last_non_comment_token(n);
            let ends_with = t.token_kind(last) == Some(Tk::Semi);
            match style {
                EndStmtWithSemicolon::Always => {
                    if kind != Nk::LabelStatement && !ends_with && t.is_token(last) {
                        self.token.semicolon[last] = Some(Semicolon::Add);
                    }
                }
                EndStmtWithSemicolon::ReplaceWithNewline => {
                    if t.is_token(t.child_token(n, Tk::Semi)) && ends_with {
                        self.token.semicolon[last] = Some(Semicolon::Remove);
                        let next = t.next_token_skip_comment(n);
                        if t.is_token(next) && t.start_line(next) == t.end_line(n) {
                            self.token.semicolon[next] = Some(Semicolon::InsertNewLine);
                        }
                    }
                }
                EndStmtWithSemicolon::SameLine => {
                    if ends_with {
                        let next = t.next_token(n);
                        let last_of_line = next == 0 || t.end_line(n) != t.start_line(next);
                        if last_of_line {
                            self.token.semicolon[last] = Some(Semicolon::Remove);
                        }
                    }
                }
                EndStmtWithSemicolon::Never => {
                    if ends_with {
                        let next = t.next_token(last);
                        let correct = t.end_line(n) != t.start_line(next)
                            && !matches!(t.token_kind(next), Some(Tk::LCurly | Tk::LParen));
                        if correct {
                            self.token.semicolon[last] = Some(Semicolon::Remove);
                        }
                    }
                }
                EndStmtWithSemicolon::Keep => {}
            }
        }
    }

    fn last_non_comment_token(&self, n: usize) -> usize {
        let t = self.t;
        let token = t.last_token(n);
        match t.token_kind(token) {
            Some(Tk::ShortComment | Tk::LongComment | Tk::Shebang) => t.prev_token(token),
            _ => token,
        }
    }

    pub(super) fn semicolon_query(&mut self, n: usize, r: &mut Resolve) {
        match self.token.semicolon[n] {
            Some(Semicolon::Add) => r.next_token = NextTokenStrategy::StmtEndSemicolon,
            Some(Semicolon::Remove) => r.token = TokenStrategy::StmtEndSemicolon,
            Some(Semicolon::InsertNewLine) => r.token = TokenStrategy::NewLineBeforeToken,
            None => {}
        }
    }

    pub(super) fn doc_analyze(&mut self) {
        let t = self.t;
        for n in t.all() {
            if t.token_kind(n) == Some(Tk::ShortComment) {
                self.analyze_doc_format(n);
            }
        }
    }

    pub(super) fn doc_query(&mut self, n: usize, r: &mut Resolve) {
        if let Some(range) = self.token.ignores[n] {
            self.ignore = range;
            r.token = TokenStrategy::OriginRange;
            r.origin_range = range;
        }
    }

    /// `FormatDocAnalyze::AnalyzeDocFormat`: `---@format disable`, `disable-next`
    /// or `disable-next-line`, exactly three dashes.
    fn analyze_doc_format(&mut self, n: usize) {
        #[derive(PartialEq)]
        enum St {
            Init,
            Tag,
            Action,
        }
        let t = self.t;
        let text = t.node_text(n).as_bytes();
        let mut i = 0;
        let mut state = St::Init;
        let eat = |i: &mut usize, f: &dyn Fn(u8) -> bool| {
            let start = *i;
            while *i < text.len() && f(text[*i]) {
                *i += 1;
            }
            &text[start..*i]
        };
        while i < text.len() {
            if text[i].is_ascii_whitespace() || text[i] == 0x0b {
                eat(&mut i, &|c| c.is_ascii_whitespace() || c == 0x0b);
                continue;
            }
            match state {
                St::Init => {
                    if eat(&mut i, &|c| c == b'-').len() != 3 {
                        return;
                    }
                    state = St::Tag;
                }
                St::Tag => {
                    if text[i] != b'@' {
                        return;
                    }
                    i += 1;
                    if eat(&mut i, &|c| c.is_ascii_alphabetic()) != b"format" {
                        return;
                    }
                    state = St::Action;
                }
                St::Action => {
                    if !text[i].is_ascii_alphabetic() {
                        return;
                    }
                    match eat(&mut i, &|c| c.is_ascii_alphanumeric() || c == b'-') {
                        b"disable" => {
                            let last = t.last_token(t.parent(n));
                            self.add_ignore_range(n, last);
                        }
                        b"disable-next" => {
                            let mut next = t.next_sibling(n);
                            while next != 0 && t.token_kind(next) == Some(Tk::ShortComment) {
                                next = t.next_sibling(next);
                            }
                            if t.is_node(next) {
                                self.add_ignore_range(n, next);
                            }
                        }
                        b"disable-next-line" => {
                            let line = t.start_line(n) + 1;
                            let ignored = |node: usize| node != 0 && t.start_line(node) == line;
                            let mut end = t.next_sibling(n);
                            while ignored(end) {
                                if t.end_line(end) > line {
                                    end = t.first_child(end);
                                } else if ignored(t.next_sibling(end)) {
                                    end = t.next_sibling(end);
                                } else {
                                    break;
                                }
                            }
                            if end != 0 {
                                self.add_ignore_range(n, end);
                            }
                        }
                        // `on` and `on-next` are unfinished in C++: nothing more is read.
                        _ => return,
                    }
                }
            }
        }
    }

    fn add_ignore_range(&mut self, start: usize, end: usize) {
        let t = self.t;
        let (start, end) = (t.first_token(start), t.last_token(end));
        self.token.ignores[start] = Some((start, end));
    }
}
