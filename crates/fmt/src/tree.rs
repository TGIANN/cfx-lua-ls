//! EmmyLuaCodeStyle's syntax tree (`LuaParser/src`): its node kinds, its parser run
//! over our lexer's tokens, and its comment placement (`LuaSyntaxTree::BuildTree`).
//! The analyzers ask about this shape (flat `if`, `ForNumber`/`ForBody`, `;` inside
//! the statement, separators inside table fields, flat suffix chains), so the
//! formatter keeps it instead of reading our tree.
//!
//! Token kinds follow their lexer with non-standard symbols on, as LuaLS runs it:
//! a compound operator (`+=`) is an `=`, `?.` / `?[` are a `.` / `[`, a back-tick
//! hash is a string, `/* */` is a long comment. CfxLua forms their parser lacks
//! (`in` unpacking, set-constructor fields) are added where noted.
//!
//! Indices are positions in one arena, 0 meaning none (the file node sits there and
//! counts as none, as in C++). Offsets are bytes; an "end offset" is inclusive.

use cfx_lua_syntax::SyntaxKind as K;
use cfx_lua_syntax::lexer;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tk {
    Name,
    Number,
    String,
    LongString,
    ShortComment,
    LongComment,
    Shebang,
    And,
    Break,
    Do,
    Else,
    Elseif,
    End,
    False,
    For,
    Function,
    Goto,
    If,
    In,
    Local,
    Nil,
    Not,
    Or,
    Repeat,
    Return,
    Then,
    True,
    Until,
    While,
    Plus,
    Minus,
    Star,
    Slash,
    IDiv,
    Percent,
    Caret,
    Hash,
    Amp,
    Tilde,
    Pipe,
    Shl,
    Shr,
    Concat,
    Dots,
    Eq,
    Ne,
    Le,
    Ge,
    Lt,
    Gt,
    Assign,
    LParen,
    RParen,
    LCurly,
    RCurly,
    LBrack,
    RBrack,
    DbColon,
    Semi,
    Colon,
    Comma,
    Dot,
}

impl Tk {
    pub(crate) fn is_comment(self) -> bool {
        matches!(self, Tk::ShortComment | Tk::LongComment | Tk::Shebang)
    }

    /// `LuaParser::GetBinaryOperator` is not `OPR_NOBINOPR`.
    pub(crate) fn is_binary_op(self) -> bool {
        use Tk::*;
        matches!(
            self,
            Plus | Minus
                | Star
                | Percent
                | Caret
                | Slash
                | IDiv
                | Amp
                | Pipe
                | Tilde
                | Shl
                | Shr
                | Concat
                | Ne
                | Eq
                | Lt
                | Le
                | Gt
                | Ge
                | And
                | Or
        )
    }

    /// Left and right priority (`LuaDefine.h`).
    fn binary_priority(self) -> Option<(u8, u8)> {
        use Tk::*;
        Some(match self {
            Plus | Minus => (10, 10),
            Star | Percent | Slash | IDiv => (11, 11),
            Caret => (14, 13),
            Amp => (6, 6),
            Pipe => (4, 4),
            Tilde => (5, 5),
            Shl | Shr => (7, 7),
            Concat => (9, 8),
            Eq | Lt | Le | Ne | Gt | Ge => (3, 3),
            And => (2, 2),
            Or => (1, 1),
            _ => return None,
        })
    }

    fn is_unary_op(self) -> bool {
        matches!(self, Tk::Not | Tk::Minus | Tk::Tilde | Tk::Hash)
    }
}

const UNARY_PRIORITY: u8 = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Nk {
    File,
    Block,
    EmptyStatement,
    LocalStatement,
    LocalFunctionStatement,
    IfStatement,
    WhileStatement,
    DoStatement,
    ForStatement,
    RepeatStatement,
    FunctionStatement,
    LabelStatement,
    BreakStatement,
    ReturnStatement,
    GotoStatement,
    ExpressionStatement,
    AssignStatement,
    SuffixedExpression,
    ParExpression,
    LiteralExpression,
    StringLiteralExpression,
    ClosureExpression,
    UnaryExpression,
    BinaryExpression,
    TableExpression,
    CallExpression,
    IndexExpression,
    NameExpression,
    FunctionNameExpression,
    VarList,
    TableFieldList,
    TableField,
    TableFieldSep,
    FunctionBody,
    ParamList,
    NameDefList,
    Attribute,
    ExpressionList,
    ForNumber,
    ForList,
    ForBody,
}

impl Nk {
    /// `detail::multi_match::ExpressionMatch`.
    pub(crate) fn is_expr(self) -> bool {
        use Nk::*;
        matches!(
            self,
            SuffixedExpression
                | ParExpression
                | LiteralExpression
                | StringLiteralExpression
                | ClosureExpression
                | UnaryExpression
                | BinaryExpression
                | TableExpression
                | CallExpression
                | IndexExpression
                | NameExpression
                | FunctionNameExpression
        )
    }

    /// `detail::multi_match::StatementMatch`.
    pub(crate) fn is_stmt(self) -> bool {
        use Nk::*;
        matches!(
            self,
            EmptyStatement
                | LocalStatement
                | LocalFunctionStatement
                | IfStatement
                | WhileStatement
                | DoStatement
                | ForStatement
                | RepeatStatement
                | FunctionStatement
                | LabelStatement
                | BreakStatement
                | ReturnStatement
                | GotoStatement
                | ExpressionStatement
                | AssignStatement
        )
    }

    /// `LuaSyntaxTree::IsEatAllComment`.
    fn eats_all_comments(self) -> bool {
        matches!(self, Nk::Block | Nk::TableFieldList | Nk::File)
    }
}

/// End-of-line style found in the file (`EndOfLineType.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Eol {
    Unknown,
    Lf,
    CrLf,
    Cr,
    Mix,
}

#[derive(Debug, Clone, Copy)]
struct RawToken {
    kind: Tk,
    start: usize,
    len: usize,
}

#[derive(Debug, Clone, Copy)]
struct Token {
    kind: Tk,
    start: usize,
    len: usize,
    node: usize,
}

#[derive(Debug, Clone, Copy)]
enum Data {
    Node(Nk),
    Token(usize),
}

#[derive(Debug, Clone, Copy)]
struct Elem {
    data: Data,
    parent: usize,
    next: usize,
    prev: usize,
    first: usize,
    last: usize,
}

pub(crate) struct Tree<'a> {
    text: &'a str,
    elems: Vec<Elem>,
    tokens: Vec<Token>,
    /// The last token before each index (0: none).
    prev_tok: Vec<usize>,
    line_starts: Vec<usize>,
    pub(crate) eol: Eol,
}

/// Our tokens as theirs, comments kept, whitespace dropped. `None` when the text
/// has a token their grammar cannot hold (a lexer error).
fn convert_tokens(text: &str) -> Option<Vec<RawToken>> {
    let (tokens, errors) = lexer::tokenize(text);
    if !errors.is_empty() {
        return None;
    }
    let mut out: Vec<RawToken> = Vec::with_capacity(tokens.len() / 2);
    let mut pos = 0usize;
    let mut question: Option<usize> = None;
    for t in tokens {
        let start = pos;
        let len = u32::from(t.len) as usize;
        pos += len;
        let kind = match t.kind {
            K::WHITESPACE => continue,
            K::COMMENT => Tk::ShortComment,
            K::BLOCK_COMMENT => Tk::LongComment,
            K::SHEBANG => Tk::Shebang,
            K::NAME => Tk::Name,
            K::NUMBER => Tk::Number,
            K::STRING | K::HASH_STRING => Tk::String,
            K::LONG_STRING => Tk::LongString,
            K::AND_KW => Tk::And,
            K::BREAK_KW => Tk::Break,
            K::DO_KW => Tk::Do,
            K::ELSE_KW => Tk::Else,
            K::ELSEIF_KW => Tk::Elseif,
            K::END_KW => Tk::End,
            K::FALSE_KW => Tk::False,
            K::FOR_KW => Tk::For,
            K::FUNCTION_KW => Tk::Function,
            K::GOTO_KW => Tk::Goto,
            K::IF_KW => Tk::If,
            K::IN_KW => Tk::In,
            K::LOCAL_KW => Tk::Local,
            K::NIL_KW => Tk::Nil,
            K::NOT_KW => Tk::Not,
            K::OR_KW => Tk::Or,
            K::REPEAT_KW => Tk::Repeat,
            K::RETURN_KW => Tk::Return,
            K::THEN_KW => Tk::Then,
            K::TRUE_KW => Tk::True,
            K::UNTIL_KW => Tk::Until,
            K::WHILE_KW => Tk::While,
            K::PLUS => Tk::Plus,
            K::MINUS => Tk::Minus,
            K::STAR => Tk::Star,
            K::SLASH => Tk::Slash,
            K::SLASH2 => Tk::IDiv,
            K::PERCENT => Tk::Percent,
            K::CARET => Tk::Caret,
            K::HASH => Tk::Hash,
            K::AMP => Tk::Amp,
            K::TILDE => Tk::Tilde,
            K::PIPE => Tk::Pipe,
            K::SHL => Tk::Shl,
            K::SHR => Tk::Shr,
            K::EQ2 => Tk::Eq,
            K::NEQ => Tk::Ne,
            K::LTEQ => Tk::Le,
            K::GTEQ => Tk::Ge,
            K::LT => Tk::Lt,
            K::GT => Tk::Gt,
            K::EQ
            | K::PLUS_EQ
            | K::MINUS_EQ
            | K::STAR_EQ
            | K::SLASH_EQ
            | K::SHL_EQ
            | K::SHR_EQ
            | K::AMP_EQ
            | K::PIPE_EQ
            | K::CARET_EQ => Tk::Assign,
            K::L_PAREN => Tk::LParen,
            K::R_PAREN => Tk::RParen,
            K::L_CURLY => Tk::LCurly,
            K::R_CURLY => Tk::RCurly,
            K::L_BRACK => Tk::LBrack,
            K::R_BRACK => Tk::RBrack,
            K::COLON2 => Tk::DbColon,
            K::SEMICOLON => Tk::Semi,
            K::COLON => Tk::Colon,
            K::COMMA => Tk::Comma,
            K::DOT => Tk::Dot,
            K::DOT2 => Tk::Concat,
            K::DOT3 => Tk::Dots,
            K::QUESTION => {
                question = Some(start);
                continue;
            }
            _ => return None,
        };
        // `?.` / `?[`: one token from the `?`, as their lexer reads it.
        if let Some(q) = question.take() {
            if !matches!(kind, Tk::Dot | Tk::LBrack) {
                return None;
            }
            out.push(RawToken { kind, start: q, len: start + len - q });
            continue;
        }
        out.push(RawToken { kind, start, len });
    }
    if question.is_some() {
        return None;
    }
    Some(out)
}

/// Line starts and the end-of-line style, counting `\r\n` and `\n\r` as one break
/// (`LuaLexer::IncLinenumber`).
fn lines(text: &str) -> (Vec<usize>, Eol) {
    let b = text.as_bytes();
    let mut starts = vec![0];
    let mut eol = Eol::Unknown;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'\n' || c == b'\r' {
            i += 1;
            let this = if i < b.len() && (b[i] == b'\n' || b[i] == b'\r') && b[i] != c {
                i += 1;
                Eol::CrLf
            } else if c == b'\n' {
                Eol::Lf
            } else {
                Eol::Cr
            };
            eol = match eol {
                Eol::Unknown => this,
                Eol::Mix => Eol::Mix,
                e if e != this => Eol::Mix,
                e => e,
            };
            starts.push(i);
        } else {
            i += 1;
        }
    }
    (starts, eol)
}

/// `utf8::Utf8nLen`: code points that end within `bytes` (stops at a NUL).
pub(crate) fn utf8_len(bytes: &[u8]) -> usize {
    let (mut at, mut n) = (0usize, 0usize);
    while at < bytes.len() && bytes[at] != 0 {
        at += char_width(bytes[at]);
        n += 1;
    }
    if at > bytes.len() {
        n -= 1;
    }
    n
}

/// `utf8::Utf8nLenAtFirstLine`.
fn utf8_len_first_line(bytes: &[u8]) -> usize {
    let (mut at, mut n) = (0usize, 0usize);
    while at < bytes.len() && !matches!(bytes[at], 0 | b'\r' | b'\n') {
        at += char_width(bytes[at]);
        n += 1;
    }
    if at > bytes.len() {
        n -= 1;
    }
    n
}

fn char_width(b: u8) -> usize {
    if b & 0xf8 == 0xf0 {
        4
    } else if b & 0xf0 == 0xe0 {
        3
    } else if b & 0xe0 == 0xc0 {
        2
    } else {
        1
    }
}

// ---------------------------------------------------------------- parser

#[derive(Debug, Clone, Copy)]
enum Event {
    Start { kind: Option<Nk>, parent: usize },
    Token,
    End,
}

/// A parse error: the formatter gives up (our parser accepted the file, so this
/// means their grammar lacks a form).
struct Fail;

type PResult<T = ()> = Result<T, Fail>;

struct Parser<'t> {
    text: &'t str,
    tokens: &'t [RawToken],
    pos: usize,
    events: Vec<Event>,
}

#[derive(Clone, Copy)]
struct Marker(usize);

#[derive(Clone, Copy)]
struct Completed(usize);

impl Parser<'_> {
    fn mark(&mut self) -> Marker {
        self.events.push(Event::Start { kind: None, parent: 0 });
        Marker(self.events.len() - 1)
    }

    fn complete(&mut self, m: Marker, kind: Nk) -> Completed {
        if let Event::Start { kind: k, .. } = &mut self.events[m.0] {
            *k = Some(kind);
        }
        self.events.push(Event::End);
        Completed(m.0)
    }

    fn undo(&mut self, m: Marker) {
        if let Event::Start { kind, .. } = &mut self.events[m.0] {
            *kind = None;
        }
    }

    fn precede(&mut self, c: Completed) -> Marker {
        let m = self.mark();
        if let Event::Start { parent, .. } = &mut self.events[c.0] {
            *parent = m.0;
        }
        m
    }

    fn skip_comments(&mut self) {
        while self.pos < self.tokens.len() && self.tokens[self.pos].kind.is_comment() {
            self.pos += 1;
        }
    }

    fn current(&mut self) -> Option<Tk> {
        self.skip_comments();
        self.tokens.get(self.pos).map(|t| t.kind)
    }

    fn at(&mut self, k: Tk) -> bool {
        self.current() == Some(k)
    }

    fn lookahead(&mut self) -> Option<Tk> {
        self.skip_comments();
        self.tokens[self.pos + 1..].iter().map(|t| t.kind).find(|k| !k.is_comment())
    }

    fn current_text(&mut self) -> &str {
        self.skip_comments();
        match self.tokens.get(self.pos) {
            Some(t) => &self.text[t.start..t.start + t.len],
            None => "",
        }
    }

    fn next(&mut self) {
        self.skip_comments();
        self.events.push(Event::Token);
        self.pos += 1;
    }

    fn check_and_next(&mut self, k: Tk) -> PResult {
        if !self.at(k) {
            return Err(Fail);
        }
        self.next();
        Ok(())
    }

    fn test_and_next(&mut self, k: Tk) -> bool {
        if self.at(k) {
            self.next();
            true
        } else {
            false
        }
    }

    fn block_follow(&mut self, right_paren: bool) -> bool {
        match self.current() {
            None | Some(Tk::Else | Tk::Elseif | Tk::End | Tk::Until) => true,
            Some(Tk::RParen) => right_paren,
            _ => false,
        }
    }

    fn block(&mut self) -> PResult {
        let m = self.mark();
        while !self.block_follow(true) {
            self.statement()?;
        }
        self.complete(m, Nk::Block);
        Ok(())
    }

    fn statement(&mut self) -> PResult {
        match self.current() {
            Some(Tk::Semi) => {
                let m = self.mark();
                self.next();
                self.complete(m, Nk::EmptyStatement);
                Ok(())
            }
            Some(Tk::If) => self.if_statement(),
            Some(Tk::While) => self.while_statement(),
            Some(Tk::Do) => self.do_statement(),
            Some(Tk::For) => self.for_statement(),
            Some(Tk::Repeat) => self.repeat_statement(),
            Some(Tk::Function) => self.function_statement(),
            Some(Tk::Local) => {
                if self.lookahead() == Some(Tk::Function) {
                    self.local_function_statement()
                } else {
                    self.local_statement()
                }
            }
            Some(Tk::DbColon) => self.label_statement(),
            Some(Tk::Return) => self.return_statement(),
            Some(Tk::Break) => self.break_statement(),
            Some(Tk::Goto) => self.goto_statement(),
            _ => self.expression_statement(),
        }
    }

    fn if_statement(&mut self) -> PResult {
        let m = self.mark();
        self.test_then_block()?;
        while self.at(Tk::Elseif) {
            self.test_then_block()?;
        }
        if self.test_and_next(Tk::Else) {
            self.block()?;
        }
        self.check_and_next(Tk::End)?;
        self.test_and_next(Tk::Semi);
        self.complete(m, Nk::IfStatement);
        Ok(())
    }

    fn test_then_block(&mut self) -> PResult {
        if matches!(self.current(), Some(Tk::If | Tk::Elseif)) {
            self.next();
        }
        self.expression()?;
        self.check_and_next(Tk::Then)?;
        self.block()
    }

    fn while_statement(&mut self) -> PResult {
        let m = self.mark();
        self.check_and_next(Tk::While)?;
        self.expression()?;
        self.check_and_next(Tk::Do)?;
        self.block()?;
        self.check_and_next(Tk::End)?;
        self.test_and_next(Tk::Semi);
        self.complete(m, Nk::WhileStatement);
        Ok(())
    }

    fn do_statement(&mut self) -> PResult {
        let m = self.mark();
        self.check_and_next(Tk::Do)?;
        self.block()?;
        self.check_and_next(Tk::End)?;
        self.test_and_next(Tk::Semi);
        self.complete(m, Nk::DoStatement);
        Ok(())
    }

    fn for_statement(&mut self) -> PResult {
        let m = self.mark();
        self.check_and_next(Tk::For)?;
        if !self.at(Tk::Name) {
            return Err(Fail);
        }
        match self.lookahead() {
            Some(Tk::Assign) => self.for_number()?,
            Some(Tk::Comma | Tk::In) => self.for_list()?,
            _ => return Err(Fail),
        }
        self.test_and_next(Tk::Semi);
        self.complete(m, Nk::ForStatement);
        Ok(())
    }

    fn for_number(&mut self) -> PResult {
        let m = self.mark();
        self.check_and_next(Tk::Name)?;
        self.check_and_next(Tk::Assign)?;
        self.expression()?;
        self.check_and_next(Tk::Comma)?;
        self.expression()?;
        if self.test_and_next(Tk::Comma) {
            self.expression()?;
        }
        self.for_body()?;
        self.complete(m, Nk::ForNumber);
        Ok(())
    }

    fn for_list(&mut self) -> PResult {
        let m = self.mark();
        self.name_def_list()?;
        self.check_and_next(Tk::In)?;
        self.expression_list(None)?;
        self.for_body()?;
        self.complete(m, Nk::ForList);
        Ok(())
    }

    fn for_body(&mut self) -> PResult {
        let m = self.mark();
        self.check_and_next(Tk::Do)?;
        self.block()?;
        self.check_and_next(Tk::End)?;
        self.complete(m, Nk::ForBody);
        Ok(())
    }

    fn repeat_statement(&mut self) -> PResult {
        let m = self.mark();
        self.check_and_next(Tk::Repeat)?;
        self.block()?;
        self.check_and_next(Tk::Until)?;
        self.expression()?;
        self.test_and_next(Tk::Semi);
        self.complete(m, Nk::RepeatStatement);
        Ok(())
    }

    fn function_statement(&mut self) -> PResult {
        let m = self.mark();
        self.check_and_next(Tk::Function)?;
        self.function_name()?;
        self.function_body()?;
        self.test_and_next(Tk::Semi);
        self.complete(m, Nk::FunctionStatement);
        Ok(())
    }

    fn local_function_statement(&mut self) -> PResult {
        let m = self.mark();
        self.check_and_next(Tk::Local)?;
        self.check_and_next(Tk::Function)?;
        self.check_and_next(Tk::Name)?;
        self.function_body()?;
        self.test_and_next(Tk::Semi);
        self.complete(m, Nk::LocalFunctionStatement);
        Ok(())
    }

    fn local_statement(&mut self) -> PResult {
        let m = self.mark();
        self.check_and_next(Tk::Local)?;
        let nm = self.mark();
        loop {
            self.check_and_next(Tk::Name)?;
            self.local_attribute()?;
            if !self.test_and_next(Tk::Comma) {
                break;
            }
        }
        self.complete(nm, Nk::NameDefList);
        // CfxLua: `local a, b in t` unpacks like `= t.a, t.b`.
        if self.test_and_next(Tk::Assign) || self.test_and_next(Tk::In) {
            self.expression_list(None)?;
        }
        self.test_and_next(Tk::Semi);
        self.complete(m, Nk::LocalStatement);
        Ok(())
    }

    fn label_statement(&mut self) -> PResult {
        let m = self.mark();
        self.check_and_next(Tk::DbColon)?;
        self.check_and_next(Tk::Name)?;
        self.check_and_next(Tk::DbColon)?;
        self.complete(m, Nk::LabelStatement);
        Ok(())
    }

    fn return_statement(&mut self) -> PResult {
        let m = self.mark();
        self.check_and_next(Tk::Return)?;
        if !(self.block_follow(false) || self.at(Tk::Semi)) {
            self.expression_list(None)?;
        }
        self.test_and_next(Tk::Semi);
        self.complete(m, Nk::ReturnStatement);
        Ok(())
    }

    fn break_statement(&mut self) -> PResult {
        let m = self.mark();
        self.check_and_next(Tk::Break)?;
        self.test_and_next(Tk::Semi);
        self.complete(m, Nk::BreakStatement);
        Ok(())
    }

    fn goto_statement(&mut self) -> PResult {
        let m = self.mark();
        self.check_and_next(Tk::Goto)?;
        self.check_and_next(Tk::Name)?;
        self.test_and_next(Tk::Semi);
        self.complete(m, Nk::GotoStatement);
        Ok(())
    }

    fn expression_statement(&mut self) -> PResult {
        // `global` statements are Lua 5.5; in CfxLua `global` is a plain name.
        let m = self.mark();
        self.suffixed_expression()?;
        // CfxLua: `a, b in t` unpacks like `a, b = t.a, t.b`.
        if matches!(self.current(), Some(Tk::Assign | Tk::Comma | Tk::In)) {
            while self.test_and_next(Tk::Comma) {
                self.suffixed_expression()?;
            }
            let c = self.complete(m, Nk::VarList);
            let m = self.precede(c);
            if !self.test_and_next(Tk::Assign) {
                self.check_and_next(Tk::In)?;
            }
            self.expression_list(None)?;
            self.test_and_next(Tk::Semi);
            self.complete(m, Nk::AssignStatement);
        } else {
            self.test_and_next(Tk::Semi);
            self.complete(m, Nk::ExpressionStatement);
        }
        Ok(())
    }

    fn expression_list(&mut self, stop: Option<Tk>) -> PResult {
        let m = self.mark();
        self.expression()?;
        while self.test_and_next(Tk::Comma) {
            if stop.is_some() && self.current() == stop {
                break;
            }
            self.expression()?;
        }
        self.complete(m, Nk::ExpressionList);
        Ok(())
    }

    fn expression(&mut self) -> PResult {
        self.subexpression(0).map(|_| ())
    }

    fn subexpression(&mut self, limit: u8) -> PResult<Completed> {
        let mut cm = if self.current().is_some_and(Tk::is_unary_op) {
            let m = self.mark();
            self.next();
            self.subexpression(UNARY_PRIORITY)?;
            self.complete(m, Nk::UnaryExpression)
        } else {
            self.simple_expression()?
        };
        while let Some((left, right)) = self.current().and_then(Tk::binary_priority) {
            if left <= limit {
                break;
            }
            let m = self.precede(cm);
            self.next();
            self.subexpression(right)?;
            cm = self.complete(m, Nk::BinaryExpression);
        }
        Ok(cm)
    }

    fn simple_expression(&mut self) -> PResult<Completed> {
        match self.current() {
            Some(Tk::Number | Tk::Nil | Tk::True | Tk::False | Tk::Dots) => {
                let m = self.mark();
                self.next();
                Ok(self.complete(m, Nk::LiteralExpression))
            }
            Some(Tk::LongString | Tk::String) => {
                let m = self.mark();
                self.next();
                Ok(self.complete(m, Nk::StringLiteralExpression))
            }
            Some(Tk::LCurly) => self.table_constructor(),
            Some(Tk::Function) => {
                let m = self.mark();
                self.next();
                self.function_body()?;
                Ok(self.complete(m, Nk::ClosureExpression))
            }
            _ => self.suffixed_expression(),
        }
    }

    fn table_constructor(&mut self) -> PResult<Completed> {
        let m = self.mark();
        self.check_and_next(Tk::LCurly)?;
        let fl = self.mark();
        while !self.at(Tk::RCurly) {
            if self.current().is_none() {
                return Err(Fail);
            }
            self.field()?;
        }
        self.complete(fl, Nk::TableFieldList);
        self.check_and_next(Tk::RCurly)?;
        Ok(self.complete(m, Nk::TableExpression))
    }

    fn field(&mut self) -> PResult {
        let m = self.mark();
        match self.current() {
            Some(Tk::Name) if self.lookahead() == Some(Tk::Assign) => self.rect_field()?,
            Some(Tk::LBrack) => self.rect_field()?,
            // CfxLua set constructor: `.name` (true) or `.name = value`.
            Some(Tk::Dot) => {
                self.next();
                self.check_and_next(Tk::Name)?;
                if self.test_and_next(Tk::Assign) {
                    self.expression()?;
                }
            }
            _ => self.expression()?,
        }
        if matches!(self.current(), Some(Tk::Comma | Tk::Semi)) {
            let m2 = self.mark();
            self.next();
            self.complete(m2, Nk::TableFieldSep);
        }
        self.complete(m, Nk::TableField);
        Ok(())
    }

    fn rect_field(&mut self) -> PResult {
        if self.at(Tk::Name) {
            self.next();
        } else {
            self.y_index()?;
        }
        self.check_and_next(Tk::Assign)?;
        self.expression()
    }

    fn function_body(&mut self) -> PResult {
        let m = self.mark();
        self.check_and_next(Tk::LParen)?;
        self.param_list()?;
        self.check_and_next(Tk::RParen)?;
        self.block()?;
        self.check_and_next(Tk::End)?;
        self.complete(m, Nk::FunctionBody);
        Ok(())
    }

    fn param_list(&mut self) -> PResult {
        let m = self.mark();
        if !self.at(Tk::RParen) {
            loop {
                match self.current() {
                    Some(Tk::Name) => self.next(),
                    Some(Tk::Dots) => {
                        self.next();
                        if self.at(Tk::Name) {
                            self.next();
                        }
                        break;
                    }
                    Some(Tk::RParen) => {}
                    _ => return Err(Fail),
                }
                if !self.test_and_next(Tk::Comma) {
                    break;
                }
            }
        }
        self.complete(m, Nk::ParamList);
        Ok(())
    }

    fn suffixed_expression(&mut self) -> PResult<Completed> {
        let m = self.mark();
        let cm = self.primary_expression()?;
        let mut suffix = false;
        loop {
            match self.current() {
                Some(Tk::Dot) => self.field_sel()?,
                Some(Tk::LBrack) => self.y_index()?,
                Some(Tk::Colon) => {
                    self.field_sel()?;
                    self.function_call_args()?;
                }
                Some(Tk::LParen | Tk::LongString | Tk::String | Tk::LCurly) => self.function_call_args()?,
                _ => break,
            }
            suffix = true;
        }
        if suffix {
            Ok(self.complete(m, Nk::SuffixedExpression))
        } else {
            self.undo(m);
            Ok(cm)
        }
    }

    fn function_call_args(&mut self) -> PResult {
        let m = self.mark();
        match self.current() {
            Some(Tk::LParen) => {
                self.next();
                if !self.at(Tk::RParen) {
                    self.expression_list(Some(Tk::RParen))?;
                }
                self.check_and_next(Tk::RParen)?;
            }
            Some(Tk::LCurly) => {
                self.table_constructor()?;
            }
            Some(Tk::LongString | Tk::String) => {
                let sm = self.mark();
                self.next();
                self.complete(sm, Nk::StringLiteralExpression);
            }
            _ => return Err(Fail),
        }
        self.complete(m, Nk::CallExpression);
        Ok(())
    }

    fn field_sel(&mut self) -> PResult {
        let m = self.mark();
        self.next();
        self.check_and_next(Tk::Name)?;
        self.complete(m, Nk::IndexExpression);
        Ok(())
    }

    fn y_index(&mut self) -> PResult {
        let m = self.mark();
        self.check_and_next(Tk::LBrack)?;
        self.expression()?;
        self.check_and_next(Tk::RBrack)?;
        self.complete(m, Nk::IndexExpression);
        Ok(())
    }

    fn function_name(&mut self) -> PResult {
        let m = self.mark();
        self.check_and_next(Tk::Name)?;
        while self.at(Tk::Dot) {
            self.field_sel()?;
        }
        if self.at(Tk::Colon) {
            self.field_sel()?;
        }
        self.complete(m, Nk::FunctionNameExpression);
        Ok(())
    }

    fn local_attribute(&mut self) -> PResult {
        let m = self.mark();
        if self.test_and_next(Tk::Lt) {
            if !self.at(Tk::Name) || !matches!(self.current_text(), "const" | "close") {
                return Err(Fail);
            }
            self.next();
            self.check_and_next(Tk::Gt)?;
            self.complete(m, Nk::Attribute);
            return Ok(());
        }
        self.undo(m);
        Ok(())
    }

    fn primary_expression(&mut self) -> PResult<Completed> {
        let m = self.mark();
        match self.current() {
            Some(Tk::LParen) => {
                self.next();
                self.expression()?;
                self.check_and_next(Tk::RParen)?;
                Ok(self.complete(m, Nk::ParExpression))
            }
            Some(Tk::Name) => {
                self.next();
                Ok(self.complete(m, Nk::NameExpression))
            }
            _ => Err(Fail),
        }
    }

    fn name_def_list(&mut self) -> PResult {
        let m = self.mark();
        self.check_and_next(Tk::Name)?;
        while self.test_and_next(Tk::Comma) {
            self.check_and_next(Tk::Name)?;
        }
        self.complete(m, Nk::NameDefList);
        Ok(())
    }
}

// ---------------------------------------------------------------- tree build

struct Builder<'t> {
    raw: &'t [RawToken],
    line_starts: &'t [usize],
    pos: usize,
    elems: Vec<Elem>,
    tokens: Vec<Token>,
    stack: Vec<usize>,
}

impl Builder<'_> {
    fn line(&self, offset: usize) -> usize {
        line_of(self.line_starts, offset)
    }

    fn link(&mut self, current: usize) {
        if let Some(&parent) = self.stack.last() {
            let last = self.elems[parent].last;
            if self.elems[parent].first == 0 {
                self.elems[parent].first = current;
            } else {
                self.elems[last].next = current;
                self.elems[current].prev = last;
            }
            self.elems[parent].last = current;
            self.elems[current].parent = parent;
        }
    }

    fn build_node(&mut self, kind: Nk) {
        let current = self.elems.len();
        self.elems.push(Elem { data: Data::Node(kind), parent: 0, next: 0, prev: 0, first: 0, last: 0 });
        self.link(current);
        self.stack.push(current);
    }

    fn eat_token(&mut self) {
        let t = self.raw[self.pos];
        self.pos += 1;
        let current = self.elems.len();
        self.tokens.push(Token { kind: t.kind, start: t.start, len: t.len, node: current });
        self.elems.push(Elem {
            data: Data::Token(self.tokens.len() - 1),
            parent: 0,
            next: 0,
            prev: 0,
            first: 0,
            last: 0,
        });
        self.link(current);
    }

    fn eat_comments(&mut self) {
        while self.pos < self.raw.len() && self.raw[self.pos].kind.is_comment() {
            self.eat_token();
        }
    }

    fn eat_inline_comment(&mut self) {
        if self.pos > 0 && self.pos < self.raw.len() && self.raw[self.pos].kind.is_comment() {
            let prev = self.raw[self.pos - 1];
            let prev_end = prev.start + prev.len.max(1) - 1;
            if self.line(prev_end) == self.line(self.raw[self.pos].start) {
                self.eat_token();
            }
        }
    }

    fn start_node(&mut self, kind: Nk) {
        if !kind.eats_all_comments() {
            self.eat_comments();
        }
        self.build_node(kind);
    }

    fn finish_node(&mut self) {
        if let Some(&top) = self.stack.last() {
            match self.elems[top].data {
                Data::Node(k) if k.eats_all_comments() => self.eat_comments(),
                _ => {
                    if self.pos < self.raw.len() && self.pos > 0 {
                        self.eat_inline_comment();
                    }
                }
            }
            self.stack.pop();
        }
    }
}

pub(crate) fn line_of(starts: &[usize], offset: usize) -> usize {
    starts.partition_point(|&s| s <= offset).saturating_sub(1)
}

impl<'a> Tree<'a> {
    /// Their tree for `text`, or `None` when their grammar does not cover it. Call
    /// only on text our parser accepted.
    pub(crate) fn new(text: &'a str) -> Option<Tree<'a>> {
        let raw = convert_tokens(text)?;
        let mut p = Parser { text, tokens: &raw, pos: 0, events: Vec::new() };
        p.block().ok()?;
        if p.current().is_some() {
            return None;
        }
        let mut events = p.events;
        let (line_starts, eol) = lines(text);
        let mut b = Builder {
            raw: &raw,
            line_starts: &line_starts,
            pos: 0,
            elems: Vec::with_capacity(events.len()),
            tokens: Vec::with_capacity(raw.len()),
            stack: Vec::new(),
        };
        b.start_node(Nk::File);
        let mut parents: Vec<Nk> = Vec::new();
        for i in 0..events.len() {
            match events[i] {
                Event::Start { kind: None, .. } => {}
                Event::Start { kind: Some(kind), parent } => {
                    parents.push(kind);
                    let mut at = parent;
                    while at > 0 {
                        let Event::Start { kind, parent } = events[at] else { break };
                        parents.extend(kind);
                        events[at] = Event::Start { kind: None, parent };
                        at = parent;
                    }
                    for &k in parents.iter().rev() {
                        b.start_node(k);
                    }
                    parents.clear();
                }
                Event::Token => {
                    b.eat_comments();
                    b.eat_token();
                    b.eat_inline_comment();
                }
                Event::End => b.finish_node(),
            }
        }
        b.finish_node();
        let (elems, tokens) = (b.elems, b.tokens);
        let mut prev_tok = vec![0; elems.len() + 1];
        for i in 1..=elems.len() {
            prev_tok[i] = if matches!(elems[i - 1].data, Data::Token(_)) && i - 1 != 0 { i - 1 } else { prev_tok[i - 1] };
        }
        Some(Tree { text, elems, tokens, prev_tok, line_starts, eol })
    }

    pub(crate) fn text(&self) -> &'a str {
        self.text
    }

    /// Every node and token after the file node, in document order.
    pub(crate) fn all(&self) -> std::ops::Range<usize> {
        1..self.elems.len()
    }

    pub(crate) fn root(&self) -> usize {
        1
    }

    pub(crate) fn is_node(&self, n: usize) -> bool {
        n != 0 && n < self.elems.len() && matches!(self.elems[n].data, Data::Node(_))
    }

    pub(crate) fn is_token(&self, n: usize) -> bool {
        n != 0 && n < self.elems.len() && matches!(self.elems[n].data, Data::Token(_))
    }

    pub(crate) fn node_kind(&self, n: usize) -> Option<Nk> {
        match self.elems.get(n)?.data {
            Data::Node(k) if n != 0 => Some(k),
            _ => None,
        }
    }

    pub(crate) fn token_kind(&self, n: usize) -> Option<Tk> {
        match self.elems.get(n)?.data {
            Data::Token(i) if n != 0 => Some(self.tokens[i].kind),
            _ => None,
        }
    }

    pub(crate) fn is_expr(&self, n: usize) -> bool {
        self.node_kind(n).is_some_and(Nk::is_expr)
    }

    pub(crate) fn parent(&self, n: usize) -> usize {
        self.elems.get(n).map_or(0, |e| e.parent)
    }

    pub(crate) fn next_sibling(&self, n: usize) -> usize {
        self.elems.get(n).map_or(0, |e| e.next)
    }

    pub(crate) fn prev_sibling(&self, n: usize) -> usize {
        self.elems.get(n).map_or(0, |e| e.prev)
    }

    pub(crate) fn first_child(&self, n: usize) -> usize {
        self.elems.get(n).map_or(0, |e| e.first)
    }

    pub(crate) fn last_child(&self, n: usize) -> usize {
        self.elems.get(n).map_or(0, |e| e.last)
    }

    pub(crate) fn children(&self, n: usize) -> impl Iterator<Item = usize> + '_ {
        let mut c = self.first_child(n);
        std::iter::from_fn(move || {
            (c != 0).then(|| {
                let r = c;
                c = self.next_sibling(c);
                r
            })
        })
    }

    pub(crate) fn child_node(&self, n: usize, k: Nk) -> usize {
        self.children(n).find(|&c| self.node_kind(c) == Some(k)).unwrap_or(0)
    }

    pub(crate) fn child_nodes(&self, n: usize, k: Nk) -> Vec<usize> {
        self.children(n).filter(|&c| self.node_kind(c) == Some(k)).collect()
    }

    pub(crate) fn child_expr(&self, n: usize) -> usize {
        self.children(n).find(|&c| self.is_expr(c)).unwrap_or(0)
    }

    pub(crate) fn child_exprs(&self, n: usize) -> Vec<usize> {
        self.children(n).filter(|&c| self.is_expr(c)).collect()
    }

    pub(crate) fn last_child_expr(&self, n: usize) -> usize {
        let mut c = self.last_child(n);
        while c != 0 && !self.is_expr(c) {
            c = self.prev_sibling(c);
        }
        c
    }

    pub(crate) fn last_child_node(&self, n: usize, k: Nk) -> usize {
        let mut c = self.last_child(n);
        while c != 0 && self.node_kind(c) != Some(k) {
            c = self.prev_sibling(c);
        }
        c
    }

    pub(crate) fn child_token(&self, n: usize, k: Tk) -> usize {
        self.children(n).find(|&c| self.token_kind(c) == Some(k)).unwrap_or(0)
    }

    pub(crate) fn child_tokens(&self, n: usize, k: Tk) -> Vec<usize> {
        self.children(n).filter(|&c| self.token_kind(c) == Some(k)).collect()
    }

    pub(crate) fn first_token(&self, n: usize) -> usize {
        if n == 0 || n >= self.elems.len() {
            return 0;
        }
        let mut c = n;
        while self.is_node(c) {
            c = self.first_child(c);
        }
        c
    }

    pub(crate) fn last_token(&self, n: usize) -> usize {
        if n == 0 || n >= self.elems.len() {
            return 0;
        }
        let mut c = n;
        while self.is_node(c) {
            c = self.last_child(c);
        }
        c
    }

    fn token_index(&self, n: usize) -> usize {
        match self.elems[n].data {
            Data::Token(i) => i,
            Data::Node(_) => unreachable!("not a token"),
        }
    }

    pub(crate) fn prev_token(&self, n: usize) -> usize {
        if n == 0 || n >= self.elems.len() {
            return 0;
        }
        if self.is_token(n) {
            let i = self.token_index(n);
            return if i != 0 { self.tokens[i - 1].node } else { 0 };
        }
        self.prev_tok[n]
    }

    pub(crate) fn next_token(&self, n: usize) -> usize {
        if n == 0 || n >= self.elems.len() {
            return 0;
        }
        let token = if self.is_token(n) {
            n
        } else {
            match self.last_token(n) {
                0 => self.prev_tok[n],
                t => t,
            }
        };
        if token == 0 {
            return 0;
        }
        let i = self.token_index(token);
        self.tokens.get(i + 1).map_or(0, |t| t.node)
    }

    pub(crate) fn next_token_skip_comment(&self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        let mut next = self.next_token(n);
        while matches!(self.token_kind(next), Some(Tk::ShortComment | Tk::LongComment)) {
            next = self.next_token(next);
        }
        next
    }

    pub(crate) fn start_offset(&self, n: usize) -> usize {
        if n == 0 || n >= self.elems.len() {
            return 0;
        }
        if self.is_token(n) {
            return self.tokens[self.token_index(n)].start;
        }
        match self.first_token(n) {
            0 => match Some(self.prev_tok[n]).filter(|&p| p != 0) {
                Some(p) => {
                    let t = self.tokens[self.token_index(p)];
                    t.start + t.len
                }
                None => 0,
            },
            t => self.tokens[self.token_index(t)].start,
        }
    }

    /// Inclusive, as `LuaSyntaxTree::GetEndOffset` (an empty node gives the end of
    /// the token before it, exclusive).
    pub(crate) fn end_offset(&self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        let token = if n >= self.elems.len() {
            self.elems.len().saturating_sub(1)
        } else if self.is_node(n) {
            match self.last_token(n) {
                0 => {
                    return match Some(self.prev_tok[n]).filter(|&p| p != 0) {
                        Some(p) => {
                            let t = self.tokens[self.token_index(p)];
                            t.start + t.len
                        }
                        None => 0,
                    };
                }
                t => t,
            }
        } else {
            n
        };
        if token == 0 || !self.is_token(token) {
            return 0;
        }
        let t = self.tokens[self.token_index(token)];
        if t.len != 0 { t.start + t.len - 1 } else { t.start }
    }

    /// `(start, length)`.
    pub(crate) fn text_range(&self, n: usize) -> (usize, usize) {
        if self.is_token(n) {
            let t = self.tokens[self.token_index(n)];
            return (t.start, t.len);
        }
        if n == 0 {
            return (0, 0);
        }
        let (start, end) = (self.start_offset(n), self.end_offset(n));
        if end >= start { (start, end - start + 1) } else { (start, 0) }
    }

    pub(crate) fn node_text(&self, n: usize) -> &'a str {
        let (start, len) = self.text_range(n);
        if len == 0 { "" } else { &self.text[start..start + len] }
    }

    pub(crate) fn utf8_len(&self, n: usize) -> usize {
        utf8_len(self.node_text(n).as_bytes())
    }

    pub(crate) fn first_line_width(&self, n: usize) -> usize {
        utf8_len_first_line(self.node_text(n).as_bytes())
    }

    pub(crate) fn line(&self, offset: usize) -> usize {
        line_of(&self.line_starts, offset)
    }

    pub(crate) fn column(&self, offset: usize) -> usize {
        let start = self.line_starts[self.line(offset)];
        if offset > start { utf8_len(&self.text.as_bytes()[start..offset]) } else { 0 }
    }

    pub(crate) fn start_line(&self, n: usize) -> usize {
        self.line(self.start_offset(n))
    }

    pub(crate) fn end_line(&self, n: usize) -> usize {
        self.line(self.end_offset(n))
    }

    pub(crate) fn start_col(&self, n: usize) -> usize {
        self.column(self.start_offset(n))
    }

    pub(crate) fn end_col(&self, n: usize) -> usize {
        self.column(self.end_offset(n))
    }

    pub(crate) fn is_single_line(&self, n: usize) -> bool {
        n != 0 && self.start_line(n) == self.end_line(n)
    }

    /// `LuaSource::CheckNonUniformCharBefore`: a character past Latin Extended-B
    /// between the line start and `offset`.
    pub(crate) fn non_uniform_char_before(&self, offset: usize) -> bool {
        let start = self.line_starts[self.line(offset)];
        let bytes = self.text.as_bytes();
        let mut i = start;
        while i < offset {
            let w = char_width(bytes[i]).min(bytes.len() - i);
            let cp = match std::str::from_utf8(&bytes[i..i + w]) {
                Ok(s) => s.chars().next().map_or(0, |c| c as u32),
                Err(_) => u32::MAX,
            };
            if cp > 0x24f {
                return true;
            }
            i += w;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Indented kinds, tokens by text.
    fn dump(text: &str) -> String {
        let t = Tree::new(text).expect("tree");
        let mut out = String::new();
        let mut stack = vec![(t.root(), 0usize)];
        while let Some((n, depth)) = stack.pop() {
            out.push_str(&"  ".repeat(depth));
            match t.node_kind(n) {
                Some(k) => out.push_str(&format!("{k:?}\n")),
                None => out.push_str(&format!("{:?} {:?}\n", t.token_kind(n).unwrap(), t.node_text(n))),
            }
            let children: Vec<_> = t.children(n).collect();
            for &c in children.iter().rev() {
                stack.push((c, depth + 1));
            }
        }
        out
    }

    #[test]
    fn shape_matches_their_parser() {
        let d = dump("-- a\nlocal x <const> = f.y:z(1, 'a') -- b\nif x then end;\n");
        assert_eq!(
            d,
            r#"Block
  ShortComment "-- a"
  LocalStatement
    Local "local"
    NameDefList
      Name "x"
      Attribute
        Lt "<"
        Name "const"
        Gt ">"
    Assign "="
    ExpressionList
      SuffixedExpression
        NameExpression
          Name "f"
        IndexExpression
          Dot "."
          Name "y"
        IndexExpression
          Colon ":"
          Name "z"
        CallExpression
          LParen "("
          ExpressionList
            LiteralExpression
              Number "1"
            Comma ","
            StringLiteralExpression
              String "'a'"
          RParen ")"
          ShortComment "-- b"
  IfStatement
    If "if"
    NameExpression
      Name "x"
    Then "then"
    Block
    End "end"
    Semi ";"
"#
        );
    }

    #[test]
    fn cfx_forms() {
        let d = dump("a?.b += 1\nlocal p, q in t\nx = { .a, .b = 2 }\n");
        assert!(d.contains("Dot \"?.\""), "{d}");
        assert!(d.contains("Assign \"+=\""), "{d}");
        assert!(d.contains("In \"in\""), "{d}");
        assert!(d.contains("TableField\n            Dot \".\"\n            Name \"a\""), "{d}");
    }

    #[test]
    fn lines_and_eol() {
        assert_eq!(lines("a\r\nb\n\rc\rd"), (vec![0, 3, 6, 8], Eol::Mix));
        assert_eq!(lines("a\r\nb\r\n").1, Eol::CrLf);
        assert_eq!(lines("x").1, Eol::Unknown);
        assert_eq!(utf8_len("aç".as_bytes()), 2);
        assert_eq!(utf8_len(&"aç".as_bytes()[..2]), 1);
    }
}
