//! Tree helpers shared by the features.

use crate::{TextRange, TextSize};
use cfx_lua_syntax::SyntaxKind::{self, *};
use cfx_lua_syntax::{CfxLua, SyntaxNode, SyntaxToken};
use rowan::{GreenNodeData, Language, NodeOrToken};

/// From the first to the last non-trivia token: a statement's node starts at the
/// comments above it, and a node can end in trivia before an empty child node
/// (`local function` with no body). Walks the green tree: no red nodes are allocated.
///
/// Only nodes that take comments can start with trivia; the others skip that walk,
/// which would follow the whole left spine of a chain like `f(...)(...)...` and
/// make the chain quadratic.
pub(crate) fn trimmed(node: &SyntaxNode) -> Option<TextRange> {
    let r = node.text_range();
    let green = node.green();
    let trail = trivia_len(&green, true)?;
    let kind = node.kind();
    let lead = if kind.attaches_comments() || matches!(kind, BLOCK | SOURCE_FILE) {
        trivia_len(&green, false)?
    } else {
        TextSize::from(0)
    };
    Some(TextRange::new(r.start() + lead, r.end() - trail))
}

/// Length of the trivia before the first real token (or after the last one, `from_end`);
/// `None` when the node holds no real token. Iterative: chains nest without limit.
fn trivia_len(green: &GreenNodeData, from_end: bool) -> Option<TextSize> {
    let mut skipped = TextSize::from(0);
    let mut stack = vec![green.children()];
    while let Some(children) = stack.last_mut() {
        let next = if from_end { children.next_back() } else { children.next() };
        match next {
            None => {
                stack.pop();
            }
            Some(NodeOrToken::Token(t)) if CfxLua::kind_from_raw(t.kind()).is_trivia() => skipped += t.text_len(),
            Some(NodeOrToken::Token(_)) => return Some(skipped),
            Some(NodeOrToken::Node(n)) => stack.push(n.children()),
        }
    }
    None
}

/// A quoted string's contents, `None` when it is not closed by the same quote
/// (an unterminated string the parser recovered).
pub(crate) fn unquote(s: &str) -> Option<&str> {
    let q = s.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    s[1..].strip_suffix(q).filter(|_| s.len() >= 2)
}

pub(crate) fn token(node: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxToken> {
    tokens(node).find(|t| t.kind() == kind)
}

pub(crate) fn tokens(node: &SyntaxNode) -> impl Iterator<Item = SyntaxToken> + use<> {
    node.children_with_tokens().filter_map(|e| e.into_token())
}

/// The function of a `function` / `local function` statement: from the `function`
/// keyword to the end, and its body.
pub(crate) fn statement_function(stat: &SyntaxNode) -> Option<(TextRange, Option<SyntaxNode>)> {
    let kw = token(stat, FUNCTION_KW)?.text_range();
    let end = trimmed(stat)?.end();
    Some((TextRange::new(kw.start(), end.max(kw.end())), stat.children().find(|c| c.kind() == FUNC_BODY)))
}

/// One clause of an `if` statement: `if`, each `elseif` and `else`, from its keyword
/// to the next clause's keyword or to `end` (LuaLS block ranges).
pub(crate) struct IfClause {
    pub(crate) keyword: SyntaxToken,
    /// The `ELSEIF_CLAUSE` / `ELSE_CLAUSE` node; the `IF_STAT` for the first clause.
    pub(crate) owner: SyntaxNode,
    pub(crate) range: TextRange,
}

pub(crate) fn if_clauses(stat: &SyntaxNode) -> Vec<IfClause> {
    let Some(whole) = trimmed(stat) else { return Vec::new() };
    let mut heads: Vec<(SyntaxToken, SyntaxNode)> = Vec::new();
    if let Some(kw) = token(stat, IF_KW) {
        heads.push((kw, stat.clone()));
    }
    for clause in stat.children().filter(|c| matches!(c.kind(), ELSEIF_CLAUSE | ELSE_CLAUSE)) {
        if let Some(kw) = tokens(&clause).find(|t| matches!(t.kind(), ELSEIF_KW | ELSE_KW)) {
            heads.push((kw, clause));
        }
    }
    let end = token(stat, END_KW).map_or(whole.end(), |t| t.text_range().start());
    let starts: Vec<TextSize> = heads.iter().map(|(kw, _)| kw.text_range().start()).collect();
    heads
        .into_iter()
        .enumerate()
        .map(|(i, (keyword, owner))| {
            let start = starts[i];
            let finish = starts.get(i + 1).copied().unwrap_or(end);
            IfClause { keyword, owner, range: TextRange::new(start, finish.max(start)) }
        })
        .collect()
}
