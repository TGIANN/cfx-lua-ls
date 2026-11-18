//! Lossless CfxLua (FiveM's Lua 5.4) syntax tree.
//!
//! `parse` never fails: it always returns a tree whose text equals the input,
//! plus the syntax errors found on the way.

mod build;
mod grammar;
mod kind;
pub mod lexer;
mod parser;
mod validate;

pub use kind::{CfxLua, SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken};
pub use rowan::{GreenNode, TextRange, TextSize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxError {
    pub range: TextRange,
    /// Stable code, LuaLS error type names where one exists (`MISS_NAME`,
    /// `BREAK_OUTSIDE`, ...). Future LSP diagnostic code.
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct Parse {
    green: GreenNode,
    errors: Vec<SyntaxError>,
    /// Deeper than `DEEP_TREE`: dropped iteratively (see `Drop`).
    deep: bool,
}

/// rowan frees a green tree recursively, one stack frame per level. Long
/// `a.b.b...` / `a + a + ...` chains are valid Lua and nest without limit.
const DEEP_TREE: u32 = 1000;

impl Drop for Parse {
    /// Takes a deep tree apart top-down: each node's children are held on a heap
    /// stack while the node itself is freed, so no free recurses. Trees cloned out
    /// of `green()` / `syntax()` still free recursively when they go last.
    fn drop(&mut self) {
        if !self.deep {
            return;
        }
        let root = std::mem::replace(&mut self.green, GreenNode::new(SyntaxKind::SOURCE_FILE.into(), []));
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            stack.extend(node.children().filter_map(|c| c.into_node()).map(|n| n.to_owned()));
        }
    }
}

impl Parse {
    pub fn syntax(&self) -> SyntaxNode {
        SyntaxNode::new_root(self.green.clone())
    }

    pub fn green(&self) -> &GreenNode {
        &self.green
    }

    pub fn errors(&self) -> &[SyntaxError] {
        &self.errors
    }

    /// Indented tree dump, used by tests.
    pub fn debug_tree(&self) -> String {
        format!("{:#?}", self.syntax())
    }
}

pub fn parse(text: &str) -> Parse {
    let (tokens, lex_errors) = lexer::tokenize(text);
    let kinds: Vec<SyntaxKind> = tokens.iter().map(|t| t.kind).filter(|k| !k.is_trivia()).collect();
    let mut p = parser::Parser::new(&kinds);
    grammar::source_file(&mut p);
    let (green, mut errors, depth) = build::build(text, &tokens, p.events, p.errors);
    validate::validate(&green, &mut errors);
    errors.extend(lex_errors.into_iter().map(|e| SyntaxError { range: e.range, code: e.code, message: e.message }));
    errors.sort_by_key(|e| e.range.start());
    Parse { green, errors, deep: depth > DEEP_TREE }
}

#[cfg(test)]
mod tests;
