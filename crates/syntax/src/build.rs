//! Turns parser events plus the full token stream (trivia included) into a
//! lossless rowan green tree.
//!
//! Trivia placement:
//! - Trivia before a node normally stays outside it, so node ranges start at
//!   their first real token.
//! - Comments directly above a statement or table field (no blank line in
//!   between) go inside that node, so LuaCATS `---@` blocks belong to the
//!   declaration they describe.
//! - BLOCK has no opening token; trivia before its first statement is left for
//!   that statement to claim.

use crate::SyntaxError;
use crate::SyntaxKind::{self, *};
use crate::lexer::Token;
use crate::parser::{Event, ParseError};
use rowan::{GreenNode, GreenToken, NodeOrToken, TextRange, TextSize};
use rustc_hash::FxHashMap;

/// Returns the tree, the errors and the tree's depth.
pub(crate) fn build(
    text: &str,
    tokens: &[Token],
    mut events: Vec<Event>,
    parse_errors: Vec<ParseError>,
) -> (GreenNode, Vec<SyntaxError>, u32) {
    let mut b = Builder {
        text,
        tokens,
        idx: 0,
        offset: 0.into(),
        last_end: 0.into(),
        inner: Tree::default(),
        errors: Vec::new(),
        depth: 0,
        max_depth: 0,
    };
    let mut chain: Vec<SyntaxKind> = Vec::new();
    // Ranges of the non-trivia tokens, for errors that name a token span.
    let real: Vec<TextRange> = if parse_errors.iter().any(|e| e.span.is_some()) {
        let mut offset = TextSize::from(0);
        tokens
            .iter()
            .filter_map(|t| {
                let range = TextRange::at(offset, t.len);
                offset += t.len;
                (!t.kind.is_trivia()).then_some(range)
            })
            .collect()
    } else {
        Vec::new()
    };
    let mut parse_errors: Vec<Option<ParseError>> = parse_errors.into_iter().map(Some).collect();

    for i in 0..events.len() {
        match std::mem::replace(&mut events[i], Event::Tombstone) {
            Event::Start { kind, forward_parent } => {
                chain.push(kind);
                let mut idx = i;
                let mut fp = forward_parent;
                while let Some(delta) = fp {
                    idx += delta as usize;
                    fp = match std::mem::replace(&mut events[idx], Event::Tombstone) {
                        Event::Start { kind, forward_parent } => {
                            chain.push(kind);
                            forward_parent
                        }
                        _ => unreachable!(),
                    };
                }
                let outer = *chain.last().unwrap();
                if b.depth > 0 && outer != BLOCK {
                    b.emit_trivia_before(outer);
                }
                for kind in chain.drain(..).rev() {
                    b.inner.start_node(kind);
                    b.depth += 1;
                    b.max_depth = b.max_depth.max(b.depth);
                }
            }
            Event::Finish => {
                if b.depth == 1 {
                    b.emit_all_trivia();
                }
                b.inner.finish_node();
                b.depth -= 1;
            }
            Event::Token => {
                b.emit_all_trivia();
                b.emit_token();
            }
            Event::Error(index) => {
                let Some(ParseError { code, message, span }) = parse_errors[index as usize].take() else { continue };
                // A span covers its tokens. Otherwise an error directly followed by a token
                // (err_and_bump) covers that token, and any other sits at the end of the last
                // real token ("expected ...").
                let range = match (span, events.get(i + 1)) {
                    (Some((from, to)), _) => span_range(&real, from as usize, to as usize, b.last_end),
                    (None, Some(Event::Token)) => b.next_real_token_range(),
                    _ => TextRange::empty(b.last_end),
                };
                b.errors.push(SyntaxError { range, code, message });
            }
            Event::Tombstone => {}
        }
    }
    (b.inner.finish(), b.errors, b.max_depth)
}

struct Builder<'a> {
    text: &'a str,
    tokens: &'a [Token],
    idx: usize,
    offset: TextSize,
    last_end: TextSize,
    inner: Tree<'a>,
    errors: Vec<SyntaxError>,
    depth: u32,
    max_depth: u32,
}

impl Builder<'_> {
    fn emit_token(&mut self) {
        let Some(tok) = self.tokens.get(self.idx) else { return };
        let range = TextRange::at(self.offset, tok.len);
        self.inner.token(tok.kind, &self.text[range]);
        self.offset += tok.len;
        self.idx += 1;
        if !tok.kind.is_trivia() {
            self.last_end = self.offset;
        }
    }

    fn emit_all_trivia(&mut self) {
        while self.tokens.get(self.idx).is_some_and(|t| t.kind.is_trivia()) {
            self.emit_token();
        }
    }

    fn emit_trivia_before(&mut self, kind: SyntaxKind) {
        let n = self.tokens[self.idx..].iter().take_while(|t| t.kind.is_trivia()).count();
        let attached = if kind.attaches_comments() { self.attached_trivia(n) } else { 0 };
        for _ in 0..n - attached {
            self.emit_token();
        }
    }

    /// How many of the `n` pending trivia tokens (counted from the end) are
    /// comments directly above the node, plus the whitespace between them.
    fn attached_trivia(&self, n: usize) -> usize {
        let pending = &self.tokens[self.idx..self.idx + n];
        let mut offset = self.offset;
        let texts: Vec<(SyntaxKind, &str)> = pending
            .iter()
            .map(|t| {
                let range = TextRange::at(offset, t.len);
                offset += t.len;
                (t.kind, &self.text[range])
            })
            .collect();
        let at_file_start = self.last_end == TextSize::from(0);
        // A comment on the same line as the previous real token (`a = 1, -- note`)
        // belongs to what came before, not to the next node.
        let starts_line = |i: usize| match texts[..i].last() {
            None => at_file_start,
            Some((WHITESPACE, t)) => newline_count(t) > 0 || (i == 1 && at_file_start),
            Some(_) => true,
        };
        let mut attached = 0;
        for (i, (kind, text)) in texts.iter().enumerate().rev() {
            match kind {
                COMMENT | BLOCK_COMMENT if starts_line(i) => attached = texts.len() - i,
                COMMENT | BLOCK_COMMENT => break,
                WHITESPACE if newline_count(text) >= 2 => break,
                WHITESPACE => {}
                _ => break,
            }
        }
        attached
    }

    fn next_real_token_range(&self) -> TextRange {
        let mut offset = self.offset;
        for t in &self.tokens[self.idx..] {
            if !t.kind.is_trivia() {
                return TextRange::at(offset, t.len);
            }
            offset += t.len;
        }
        TextRange::empty(self.last_end)
    }
}

/// Green tree builder. rowan's `GreenNodeBuilder` dedups small nodes in a hash
/// table whose rehash hashes every cached node recursively: a long `a.b.b...` or
/// `a + a + ...` chain (valid Lua) overflows the stack and takes quadratic time.
///
/// The same sharing is done here without recursion: tokens by kind and text,
/// nodes with at most 3 children by kind and the addresses of those children.
/// Children are shared already, so equal addresses mean equal subtrees.
#[derive(Default)]
struct Tree<'a> {
    parents: Vec<(SyntaxKind, usize)>,
    /// `true` when the element came from a cache (its address identifies it).
    children: Vec<(bool, NodeOrToken<GreenNode, GreenToken>)>,
    tokens: FxHashMap<(SyntaxKind, &'a str), GreenToken>,
    nodes: FxHashMap<(SyntaxKind, [usize; MAX_SHARED_CHILDREN]), GreenNode>,
}

/// rowan uses the same limit: bigger nodes are rarely equal.
const MAX_SHARED_CHILDREN: usize = 3;

impl<'a> Tree<'a> {
    fn start_node(&mut self, kind: SyntaxKind) {
        self.parents.push((kind, self.children.len()));
    }

    fn token(&mut self, kind: SyntaxKind, text: &'a str) {
        let token = self.tokens.entry((kind, text)).or_insert_with(|| GreenToken::new(kind.into(), text)).clone();
        self.children.push((true, NodeOrToken::Token(token)));
    }

    fn finish_node(&mut self) {
        let (kind, first) = self.parents.pop().expect("finish_node without start_node");
        let children = &self.children[first..];
        let key = (children.len() <= MAX_SHARED_CHILDREN && children.iter().all(|(shared, _)| *shared)).then(|| {
            let mut ids = [0usize; MAX_SHARED_CHILDREN];
            for (id, (_, child)) in ids.iter_mut().zip(children) {
                *id = match child {
                    NodeOrToken::Node(n) => &**n as *const _ as usize,
                    NodeOrToken::Token(t) => &**t as *const _ as usize,
                };
            }
            (kind, ids)
        });
        let new = |children: &mut Vec<(bool, NodeOrToken<GreenNode, GreenToken>)>| {
            GreenNode::new(kind.into(), children.drain(first..).map(|(_, c)| c))
        };
        let node = match key {
            Some(key) => match self.nodes.get(&key) {
                Some(node) => {
                    let node = node.clone();
                    self.children.truncate(first);
                    node
                }
                None => {
                    let node = new(&mut self.children);
                    self.nodes.insert(key, node.clone());
                    node
                }
            },
            None => new(&mut self.children),
        };
        self.children.push((key.is_some(), NodeOrToken::Node(node)));
    }

    fn finish(mut self) -> GreenNode {
        assert!(self.parents.is_empty() && self.children.len() == 1, "unbalanced tree");
        match self.children.pop() {
            Some((_, NodeOrToken::Node(node))) => node,
            _ => unreachable!("the root is a node"),
        }
    }
}

fn span_range(real: &[TextRange], from: usize, to: usize, fallback: TextSize) -> TextRange {
    match (real.get(from), to.checked_sub(1).and_then(|last| real.get(last))) {
        (Some(first), Some(last)) if from < to => first.cover(*last),
        (Some(first), _) => TextRange::empty(first.start()),
        _ => TextRange::empty(fallback),
    }
}

fn newline_count(text: &str) -> usize {
    // "\r\n" counts once.
    text.bytes().filter(|&c| c == b'\n').count().max(text.bytes().filter(|&c| c == b'\r').count())
}
