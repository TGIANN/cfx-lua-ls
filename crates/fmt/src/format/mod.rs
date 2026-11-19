//! EmmyLuaCodeStyle's `FormatState` and `FormatBuilder`: every analyzer marks
//! tokens and nodes first (`Analyze`, then `ComplexAnalyze`), then one depth-first
//! walk asks each analyzer about each element (`Query`) and writes the tokens.
//!
//! The analyzers live in the submodules as `impl Fmt` blocks over their own state,
//! called in C++'s order: space, line break, indentation, align, token, semicolon,
//! format doc.

mod align;
mod check;
mod indent;
mod line_break;
mod space;
mod token;

use crate::style::{EndOfLine, IndentStyle, LineSpace, LineSpaceType, Style};
use crate::tree::{Eol, Tk, Tree, utf8_len};
pub use check::StyleDiagnostic;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NextSpace {
    None,
    Space(usize),
    LineBreak(LineSpace),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrevSpace {
    None,
    AlignPos(usize),
    AlignRelativeIndent(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TokenStrategy {
    Origin,
    Remove,
    StringSingleQuote,
    StringDoubleQuote,
    TableSepSemicolon,
    TableSepComma,
    OriginRange,
    StmtEndSemicolon,
    NewLineBeforeToken,
    SpaceAfterCommentDash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NextTokenStrategy {
    None,
    TableAddColon,
    TableAddComma,
    StmtEndSemicolon,
    RightParentheses,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IndentStrategy {
    Relative,
    InvertRelative,
    Absolute,
}

/// `FormatResolve`: what the analyzers decided for one element.
#[derive(Debug, Clone, Copy)]
struct Resolve {
    next_space: NextSpace,
    prev_space: PrevSpace,
    left_paren_before: bool,
    token: TokenStrategy,
    next_token: NextTokenStrategy,
    indent: Option<(IndentStrategy, usize)>,
    origin_range: (usize, usize),
}

impl Default for Resolve {
    fn default() -> Resolve {
        Resolve {
            next_space: NextSpace::None,
            prev_space: PrevSpace::None,
            left_paren_before: false,
            token: TokenStrategy::Origin,
            next_token: NextTokenStrategy::None,
            indent: None,
            origin_range: (0, 0),
        }
    }
}

impl Resolve {
    fn set_indent(&mut self, indent: usize, strategy: IndentStrategy) {
        self.indent = Some((strategy, indent));
    }
}

/// `RangeFormatBuilder`: the lines asked for, widened to whole tokens. Before and
/// after them the walk only keeps count of the line width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RangeState {
    Init,
    Process,
    Finish,
}

#[derive(Debug, Clone, Copy)]
struct Range {
    start_line: usize,
    end_line: usize,
    state: RangeState,
}

#[derive(Debug, Clone, Copy)]
struct IndentState {
    node: usize,
    space: usize,
    tab: usize,
}

pub(crate) struct Fmt<'t, 'a> {
    t: &'t Tree<'a>,
    style: &'t Style,
    out: String,
    width: usize,
    indent_stack: Vec<IndentState>,
    ignore: (usize, usize),
    range: Option<Range>,
    space: space::State,
    line_break: line_break::State,
    indent: indent::State,
    align: align::State,
    token: token::State,
    /// `FormatState::Mode::Diagnostic`: `codestyle-check` instead of output.
    diagnostic: bool,
    diags: Vec<StyleDiagnostic>,
    keyed: std::collections::BTreeMap<usize, StyleDiagnostic>,
}

impl<'t, 'a> Fmt<'t, 'a> {
    pub(crate) fn new(t: &'t Tree<'a>, style: &'t Style) -> Fmt<'t, 'a> {
        let n = t.all().end;
        Fmt {
            t,
            style,
            out: String::with_capacity(t.text().len() + t.text().len() / 8),
            width: 0,
            indent_stack: Vec::new(),
            ignore: (0, 0),
            range: None,
            space: space::State::new(n),
            line_break: line_break::State::new(n),
            indent: indent::State::new(n),
            align: align::State::new(n),
            token: token::State::new(n),
            diagnostic: false,
            diags: Vec::new(),
            keyed: std::collections::BTreeMap::new(),
        }
    }

    /// `FormatBuilder::GetFormatResult`.
    pub(crate) fn run(mut self) -> String {
        self.analyze();
        self.walk();
        self.end_with_new_line(self.style.insert_final_newline);
        self.out
    }

    /// `RangeFormatBuilder::GetFormatResult`: the text that replaces lines
    /// `start..=end` as widened, or `None` when nothing is formatted (the lines
    /// touch a `---@format disable` region or hold no token).
    pub(crate) fn run_range(mut self, start_line: usize, end_line: usize) -> Option<(String, usize, usize)> {
        self.analyze();
        let t = self.t;
        for &(start, end) in self.token.ignores.iter().flatten() {
            let (first, last) = (t.start_line(start), t.end_line(end));
            if (start_line >= first && start_line <= last) || (end_line >= first && start_line < first) {
                return None;
            }
        }
        self.range = Some(Range { start_line, end_line, state: RangeState::Init });
        self.walk();
        let range = self.range.take()?;
        self.end_with_new_line(true);
        (!self.out.is_empty()).then_some((self.out, range.start_line, range.end_line))
    }

    /// Outside the range: count the width, write nothing.
    fn skipping(&self) -> bool {
        self.range.is_some_and(|r| r.state != RangeState::Process)
    }

    /// `RangeFormatBuilder::CheckRange`; true once the walk is past the range.
    fn check_range(&mut self, n: usize, r: &Resolve) -> bool {
        let t = self.t;
        let Some(range) = self.range.as_mut() else { return false };
        if !t.is_token(n) {
            return false;
        }
        let (start, end) = if r.token == TokenStrategy::OriginRange { r.origin_range } else { (n, n) };
        match range.state {
            RangeState::Init => {
                let end_line = t.end_line(end);
                if end_line >= range.start_line && end_line <= range.end_line {
                    range.state = RangeState::Process;
                    range.start_line = range.start_line.min(t.start_line(start));
                }
            }
            RangeState::Process => {
                if t.start_line(start) > range.end_line {
                    range.state = RangeState::Finish;
                    return true;
                }
                range.end_line = range.end_line.max(t.end_line(end));
            }
            RangeState::Finish => return true,
        }
        false
    }

    fn analyze(&mut self) {
        self.space_analyze();
        self.line_break_analyze();
        self.indent_analyze();
        self.align_analyze();
        self.token_analyze();
        self.semicolon_analyze();
        self.doc_analyze();

        self.space_complex_analyze();
        self.line_break_complex_analyze();
    }

    fn query(&mut self, n: usize, r: &mut Resolve) {
        self.space_query(n, r);
        self.line_break_query(n, r);
        self.indent_query(n, r);
        self.align_query(n, r);
        self.token_query(n, r);
        self.semicolon_query(n, r);
        self.doc_query(n, r);
    }

    fn is_new_line(&self) -> bool {
        self.width == 0
    }

    /// `FormatState::IsNewLine`: the output width when formatting; in diagnostic
    /// mode whether the source puts `n` first on its line.
    fn is_new_line_at(&self, n: usize) -> bool {
        if !self.diagnostic {
            return self.is_new_line();
        }
        let prev = self.t.prev_token(n);
        !self.t.is_token(prev) || self.t.end_line(prev) != self.t.start_line(n)
    }

    fn end_of_line(&self) -> &'static str {
        let eol = if self.style.detect_end_of_line {
            match self.t.eol {
                Eol::CrLf => Some(EndOfLine::CrLf),
                Eol::Cr => Some(EndOfLine::Cr),
                Eol::Lf => Some(EndOfLine::Lf),
                Eol::Unknown | Eol::Mix => None,
            }
        } else {
            Some(self.style.end_of_line)
        };
        match eol {
            Some(EndOfLine::CrLf) => "\r\n",
            Some(EndOfLine::Cr) => "\r",
            Some(EndOfLine::Lf) | None => "\n",
        }
    }

    fn indent_unit(&self) -> usize {
        match self.style.indent_style {
            IndentStyle::Space => self.style.indent_size,
            IndentStyle::Tab => self.style.tab_width,
        }
    }

    fn current_indent(&self) -> IndentState {
        self.indent_stack.last().copied().unwrap_or(IndentState { node: 0, space: 0, tab: 0 })
    }

    fn push_indent(&mut self, node: usize, strategy: IndentStrategy, indent: usize) {
        let tab_width = self.style.tab_width.max(1);
        let tabs = self.style.indent_style == IndentStyle::Tab;
        let top = self.indent_stack.last().copied();
        let state = match strategy {
            IndentStrategy::Absolute => {
                if tabs {
                    IndentState { node, space: indent % tab_width, tab: indent / tab_width }
                } else {
                    IndentState { node, space: indent, tab: 0 }
                }
            }
            _ if top.is_none() => IndentState { node, space: 0, tab: 0 },
            IndentStrategy::Relative => {
                let top = top.unwrap();
                if tabs {
                    IndentState { node, space: top.space + indent % tab_width, tab: top.tab + indent / tab_width }
                } else {
                    IndentState { node, space: top.space + indent, tab: top.tab }
                }
            }
            IndentStrategy::InvertRelative => {
                let top = top.unwrap();
                if tabs {
                    IndentState {
                        node,
                        space: top.space.saturating_sub(indent % tab_width),
                        tab: top.tab.saturating_sub(indent / tab_width),
                    }
                } else {
                    IndentState { node, space: top.space.saturating_sub(indent), tab: top.tab }
                }
            }
        };
        self.indent_stack.push(state);
    }

    /// `FormatState::DfsForeach` from the root: pre-order, children queued before
    /// the node is asked about.
    fn walk(&mut self) {
        let t = self.t;
        let mut stack: Vec<(usize, bool)> = vec![(t.root(), false)];
        let mut children: Vec<usize> = Vec::new();
        while let Some(&(n, exiting)) = stack.last() {
            if exiting {
                stack.pop();
                if self.current_indent().node == n && !self.indent_stack.is_empty() {
                    self.indent_stack.pop();
                }
                continue;
            }
            stack.last_mut().unwrap().1 = true;
            children.clear();
            children.extend(t.children(n));
            stack.extend(children.iter().rev().map(|&c| (c, false)));

            let mut r = Resolve::default();
            self.query(n, &mut r);
            if let Some((strategy, mut indent)) = r.indent {
                if indent == 0 && strategy != IndentStrategy::Absolute {
                    indent = self.indent_unit();
                }
                self.push_indent(n, strategy, indent);
            }
            if self.ignore.1 != 0 && n > self.ignore.0 && n <= self.ignore.1 {
                continue;
            }
            if self.check_range(n, &r) {
                return;
            }
            if self.diagnostic {
                self.check(n, &r);
            } else {
                self.resolve(n, &r);
            }
        }
    }

    /// `FormatBuilder::DoResolve`.
    fn resolve(&mut self, n: usize, r: &Resolve) {
        let t = self.t;
        let Some(kind) = t.token_kind(n) else { return };
        if self.is_new_line() {
            self.write_indent();
            self.indent_mark(n);
        }
        match r.prev_space {
            PrevSpace::AlignPos(pos) => {
                if pos > self.width {
                    self.write_space(pos - self.width);
                }
            }
            PrevSpace::AlignRelativeIndent(rel) => {
                let i = self.current_indent();
                let pos = rel + i.space + i.tab * self.style.tab_width;
                if pos > self.width {
                    self.write_space(pos - self.width);
                }
            }
            PrevSpace::None => {}
        }
        if r.left_paren_before {
            self.write_char('(');
        }
        match r.token {
            TokenStrategy::Origin => self.write_token(n, kind),
            TokenStrategy::StmtEndSemicolon | TokenStrategy::Remove => {}
            TokenStrategy::StringSingleQuote | TokenStrategy::StringDoubleQuote => {
                let (del, from) =
                    if r.token == TokenStrategy::StringSingleQuote { ('\'', '"') } else { ('"', '\'') };
                let text = t.node_text(n);
                if kind == Tk::String && text.len() >= 2 && text.starts_with(from) && !exist_del(del, text) {
                    self.write_char(del);
                    self.write_text(&text[1..text.len() - 1], false);
                    self.write_char(del);
                } else {
                    self.write_token(n, kind);
                }
            }
            TokenStrategy::TableSepComma => self.write_char(','),
            TokenStrategy::TableSepSemicolon => self.write_char(';'),
            TokenStrategy::OriginRange => {
                let (start, end) = r.origin_range;
                let start_offset = t.text_range(start).0;
                let (end_offset, end_len) = t.text_range(end);
                if end_offset >= start_offset {
                    let text = &t.text()[start_offset..end_offset + end_len];
                    self.write_text(text, false);
                    let next = t.next_token(end);
                    if t.is_token(next) {
                        let line = t.start_line(next).saturating_sub(t.end_line(end));
                        self.write_line(line);
                    }
                }
                return;
            }
            TokenStrategy::NewLineBeforeToken => {
                self.write_line(1);
                self.write_token(n, kind);
            }
            TokenStrategy::SpaceAfterCommentDash => {
                let text = t.node_text(n);
                let dashes = text.bytes().take_while(|&b| b == b'-').count();
                for _ in 0..dashes {
                    self.write_char('-');
                }
                self.write_char(' ');
                self.write_text(&text[dashes..], false);
            }
        }
        match r.next_token {
            NextTokenStrategy::TableAddComma => self.write_char(','),
            NextTokenStrategy::TableAddColon | NextTokenStrategy::StmtEndSemicolon => self.write_char(';'),
            NextTokenStrategy::RightParentheses => self.write_char(')'),
            NextTokenStrategy::None => {}
        }
        match r.next_space {
            NextSpace::Space(s) => self.write_space(s),
            NextSpace::LineBreak(l) => match l.kind {
                LineSpaceType::Fixed => self.write_line(l.space),
                LineSpaceType::Keep | LineSpaceType::Max | LineSpaceType::Min => {
                    let next = t.next_token(n);
                    if t.is_token(next) {
                        let (current, next_line) = (t.end_line(n), t.start_line(next));
                        if next_line > current {
                            let mut lines = next_line - current;
                            match l.kind {
                                LineSpaceType::Max => lines = lines.min(l.space),
                                LineSpaceType::Min => lines = lines.max(l.space),
                                _ => {}
                            }
                            self.write_line(lines);
                        } else {
                            self.write_line(1);
                        }
                    }
                }
            },
            NextSpace::None => {}
        }
    }

    /// `FormatBuilder::WriteSyntaxNode`.
    fn write_token(&mut self, n: usize, kind: Tk) {
        let text = self.t.node_text(n);
        if self.skipping() {
            match kind {
                Tk::String | Tk::LongString | Tk::LongComment => self.write_text(text, false),
                _ => self.width += text.len(),
            }
            return;
        }
        match kind {
            Tk::String | Tk::LongComment => self.write_text(text, false),
            // Our change: a long string keeps its trailing spaces. C++ trims them,
            // which changes the string's value.
            Tk::LongString => self.write_text(text, true),
            Tk::Number if self.style.remove_leading_zero_in_float && text.starts_with("0.") && text.len() > 1 => {
                self.width += utf8_len(&text.as_bytes()[1..]);
                self.out.push_str(&text[1..]);
            }
            _ => {
                self.width += utf8_len(text.as_bytes());
                self.out.push_str(text);
            }
        }
    }

    fn write_space(&mut self, space: usize) {
        if self.skipping() {
            self.width += space;
            return;
        }
        for _ in 0..space {
            self.out.push(' ');
        }
        self.width += space;
    }

    fn add_end_of_line(&mut self, lines: usize) {
        let eol = self.end_of_line();
        for _ in 0..lines {
            self.out.push_str(eol);
        }
        self.width = 0;
    }

    /// Ends the line, dropping the spaces before the break.
    fn write_line(&mut self, lines: usize) {
        self.write_line_keeping(lines, false);
    }

    fn write_line_keeping(&mut self, lines: usize, keep_trailing_spaces: bool) {
        if self.skipping() {
            self.width = 0;
            return;
        }
        if lines == 0 {
            return;
        }
        if !keep_trailing_spaces {
            // C++ keeps the first character even when it is a space.
            let trimmed = self.out.trim_end_matches(' ').len().max(1.min(self.out.len()));
            self.out.truncate(trimmed);
        }
        if lines > 1 && self.style.keep_indents_on_empty_lines {
            self.add_end_of_line(1);
            for _ in 1..lines {
                self.write_indent();
                self.add_end_of_line(1);
            }
        } else {
            self.add_end_of_line(lines);
        }
    }

    fn write_indent(&mut self) {
        let i = self.current_indent();
        if self.skipping() {
            self.width += i.space + i.tab * self.style.tab_width;
            return;
        }
        if self.style.indent_style == IndentStyle::Tab {
            for _ in 0..i.tab {
                self.out.push('\t');
            }
        }
        for _ in 0..i.space {
            self.out.push(' ');
        }
        self.width += i.space + i.tab * self.style.tab_width;
    }

    fn write_char(&mut self, c: char) {
        self.width += 1;
        if self.skipping() {
            return;
        }
        self.out.push(c);
    }

    /// Text that may span lines: each break becomes the file's end of line.
    fn write_text(&mut self, text: &str, keep_trailing_spaces: bool) {
        let b = text.as_bytes();
        if self.skipping() {
            // C++ counts bytes here, not characters.
            let last = b.iter().rposition(|&c| c == b'\n' || c == b'\r').map_or(0, |i| i + 1);
            if last > 0 {
                self.width = 0;
            }
            self.width += b.len() - last;
            return;
        }
        let mut last = 0;
        let mut i = 0;
        while i < b.len() {
            let c = b[i];
            if c == b'\n' || c == b'\r' {
                self.out.push_str(&text[last..i]);
                self.write_line_keeping(1, keep_trailing_spaces);
                if c == b'\r' && i + 1 < b.len() && b[i + 1] == b'\n' {
                    i += 1;
                }
                last = i + 1;
            }
            i += 1;
        }
        if last < b.len() {
            self.width += utf8_len(&b[last..]);
            self.out.push_str(&text[last..]);
        }
    }

    /// `FormatBuilder::DealEndWithNewLine`.
    fn end_with_new_line(&mut self, new_line: bool) {
        if self.out.is_empty() {
            return;
        }
        if self.out.ends_with(['\r', '\n']) {
            let trimmed = self.out.trim_end_matches(['\r', '\n']).len().max(1);
            self.out.truncate(trimmed);
        }
        if new_line {
            self.write_line(1);
        }
    }
}

/// `string_util::ExistDel`: `del` occurs unescaped inside the quotes.
fn exist_del(del: char, text: &str) -> bool {
    let inner = &text.as_bytes()[1..text.len() - 1];
    let mut escaped = false;
    for &b in inner {
        if escaped {
            escaped = false;
        } else if b == b'\\' {
            escaped = true;
        } else if b == del as u8 {
            return true;
        }
    }
    false
}
