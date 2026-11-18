//! Event-based parser infrastructure. The grammar (in `grammar.rs`) only sees
//! non-trivia tokens and emits start/finish/token events; `build.rs` later
//! weaves trivia back in and builds the lossless rowan tree.

use crate::SyntaxKind::{self, *};

#[derive(Debug)]
pub(crate) enum Event {
    Start { kind: SyntaxKind, forward_parent: Option<u32> },
    Finish,
    Token,
    /// Index into `Parser::errors`. Kept small: there are about two events per token.
    Error(u32),
    Tombstone,
}

pub(crate) struct Parser<'t> {
    tokens: &'t [SyntaxKind],
    pos: usize,
    pub(crate) events: Vec<Event>,
    pub(crate) errors: Vec<ParseError>,
    depth: u32,
}

pub(crate) struct ParseError {
    pub(crate) code: &'static str,
    pub(crate) message: String,
    /// Non-trivia token indices `[from, to)` the error covers; `None` places it by
    /// position (see `build.rs`).
    pub(crate) span: Option<(u32, u32)>,
}

/// Lua's own nesting limit (LUAI_MAXCCALLS) is 200.
const MAX_DEPTH: u32 = 200;

impl<'t> Parser<'t> {
    pub(crate) fn new(tokens: &'t [SyntaxKind]) -> Self {
        Parser { tokens, pos: 0, events: Vec::with_capacity(tokens.len() * 2), errors: Vec::new(), depth: 0 }
    }

    pub(crate) fn nth(&self, n: usize) -> SyntaxKind {
        self.tokens.get(self.pos + n).copied().unwrap_or(EOF)
    }

    /// Index of the current non-trivia token; used to detect lack of progress.
    pub(crate) fn token_pos(&self) -> usize {
        self.pos
    }

    pub(crate) fn current(&self) -> SyntaxKind {
        self.nth(0)
    }

    pub(crate) fn at(&self, kind: SyntaxKind) -> bool {
        self.current() == kind
    }

    pub(crate) fn at_any(&self, kinds: &[SyntaxKind]) -> bool {
        kinds.contains(&self.current())
    }

    pub(crate) fn bump(&mut self) {
        if self.pos < self.tokens.len() {
            self.pos += 1;
            self.events.push(Event::Token);
        }
    }

    pub(crate) fn eat(&mut self, kind: SyntaxKind) -> bool {
        if self.at(kind) {
            self.bump();
            true
        } else {
            false
        }
    }

    /// Consume `kind` or record "expected ..." without consuming anything.
    pub(crate) fn expect(&mut self, kind: SyntaxKind) -> bool {
        if self.eat(kind) {
            return true;
        }
        let code = if kind == NAME { "MISS_NAME" } else { "MISS_SYMBOL" };
        self.error(code, format!("expected {}", kind.describe()));
        false
    }

    /// Consume a name. A value keyword in its place (`local true = 1`) is reported
    /// and consumed, so parsing goes on as if it were the name.
    pub(crate) fn expect_name(&mut self) -> bool {
        if self.eat(NAME) {
            return true;
        }
        if matches!(self.current(), TRUE_KW | FALSE_KW | NIL_KW | AND_KW | OR_KW | NOT_KW) {
            let message = format!("{} is a keyword, it cannot be a name", self.current().describe());
            self.err_and_bump("KEYWORD", &message);
        } else {
            self.error("MISS_NAME", "expected a name");
        }
        false
    }

    pub(crate) fn error(&mut self, code: &'static str, message: impl Into<String>) {
        self.push_error(ParseError { code, message: message.into(), span: None });
    }

    /// An error covering the non-trivia tokens `[from, to)` (indices from `token_pos`).
    pub(crate) fn error_span(&mut self, code: &'static str, message: impl Into<String>, from: usize, to: usize) {
        self.push_error(ParseError { code, message: message.into(), span: Some((from as u32, to as u32)) });
    }

    fn push_error(&mut self, error: ParseError) {
        self.events.push(Event::Error(self.errors.len() as u32));
        self.errors.push(error);
    }

    /// Wrap the current token in an ERROR node with `message`.
    pub(crate) fn err_and_bump(&mut self, code: &'static str, message: &str) {
        let m = self.start();
        self.error(code, message);
        self.bump();
        m.complete(self, ERROR);
    }

    pub(crate) fn start(&mut self) -> Marker {
        let pos = self.events.len() as u32;
        self.events.push(Event::Tombstone);
        Marker { pos, completed: false }
    }

    /// Enter one nesting level; returns false (with an error) past Lua's limit.
    pub(crate) fn enter(&mut self) -> bool {
        if self.depth >= MAX_DEPTH {
            self.error("TOO_DEEP", "chunk has too many syntax levels");
            return false;
        }
        self.depth += 1;
        true
    }

    pub(crate) fn leave(&mut self) {
        self.depth -= 1;
    }
}

pub(crate) struct Marker {
    pos: u32,
    completed: bool,
}

impl Marker {
    pub(crate) fn complete(mut self, p: &mut Parser, kind: SyntaxKind) -> CompletedMarker {
        self.completed = true;
        match &mut p.events[self.pos as usize] {
            ev @ Event::Tombstone => *ev = Event::Start { kind, forward_parent: None },
            _ => unreachable!(),
        }
        p.events.push(Event::Finish);
        CompletedMarker { pos: self.pos, kind }
    }

    pub(crate) fn abandon(mut self, p: &mut Parser) {
        self.completed = true;
        if self.pos as usize == p.events.len() - 1 {
            p.events.pop();
        }
    }
}

impl Drop for Marker {
    fn drop(&mut self) {
        if !self.completed && !std::thread::panicking() {
            panic!("marker dropped without complete/abandon");
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct CompletedMarker {
    pos: u32,
    kind: SyntaxKind,
}

impl CompletedMarker {
    pub(crate) fn kind(self) -> SyntaxKind {
        self.kind
    }

    /// Start a new node that will become the parent of this one.
    pub(crate) fn precede(self, p: &mut Parser) -> Marker {
        let m = p.start();
        match &mut p.events[self.pos as usize] {
            Event::Start { forward_parent, .. } => *forward_parent = Some(m.pos - self.pos),
            _ => unreachable!(),
        }
        m
    }
}
