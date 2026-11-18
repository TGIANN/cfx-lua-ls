//! Checks Lua makes while compiling a chunk (`lparser.c`), run over the finished
//! tree: `break` outside a loop, `...` outside a vararg function, goto / label
//! rules, assignment to `<const>` / `<close>` locals, attributes, and the limit
//! of 200 active locals per function.
//!
//! Walks the green tree directly with an explicit stack: no allocation per node
//! (a red-tree walk doubled the parse time), and expression chains can nest
//! deeper than any call stack.

use crate::SyntaxKind::{self, *};
use crate::{CfxLua, SyntaxError};
use rowan::{Children, GreenNodeData, GreenTokenData, Language, NodeOrToken, TextRange, TextSize};

/// `MAXVARS` in lparser.c.
const MAX_LOCALS: usize = 200;
/// Hidden control variables lparser.c declares for each loop kind.
const NUMERIC_FOR_HIDDEN: usize = 3;
const GENERIC_FOR_HIDDEN: usize = 4;

pub(crate) fn validate(root: &GreenNodeData, errors: &mut Vec<SyntaxError>) {
    let root = Node { data: root, offset: TextSize::from(0) };
    let main = Func { vararg: true, locals: Vec::new(), pending: 0, limit_reported: false, first_scope: 0 };
    let mut v = Validator { funcs: vec![main], scopes: Vec::new(), statements: Vec::new(), errors };
    let mut stack: Vec<(Node, Elements)> = Vec::new();
    v.enter(root, &stack);
    stack.push((root, root.elements()));
    while let Some((_, children)) = stack.last_mut() {
        match children.next() {
            Some(NodeOrToken::Node(child)) => {
                v.enter(child, &stack);
                stack.push((child, child.elements()));
            }
            Some(NodeOrToken::Token(_)) => {}
            None => {
                let (node, _) = stack.pop().expect("frame");
                v.leave(node, &stack);
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Node<'g> {
    data: &'g GreenNodeData,
    offset: TextSize,
}

#[derive(Clone, Copy)]
struct Tok<'g> {
    data: &'g GreenTokenData,
    offset: TextSize,
}

/// Children of a node with their offsets.
#[derive(Clone)]
struct Elements<'g> {
    children: Children<'g>,
    offset: TextSize,
}

impl<'g> Iterator for Elements<'g> {
    type Item = NodeOrToken<Node<'g>, Tok<'g>>;

    fn next(&mut self) -> Option<Self::Item> {
        let offset = self.offset;
        Some(match self.children.next()? {
            NodeOrToken::Node(data) => {
                self.offset += data.text_len();
                NodeOrToken::Node(Node { data, offset })
            }
            NodeOrToken::Token(data) => {
                self.offset += data.text_len();
                NodeOrToken::Token(Tok { data, offset })
            }
        })
    }
}

impl<'g> Node<'g> {
    fn kind(self) -> SyntaxKind {
        CfxLua::kind_from_raw(self.data.kind())
    }

    fn range(self) -> TextRange {
        TextRange::at(self.offset, self.data.text_len())
    }

    fn elements(self) -> Elements<'g> {
        Elements { children: self.data.children(), offset: self.offset }
    }

    fn nodes(self) -> impl Iterator<Item = Node<'g>> + Clone {
        self.elements().filter_map(NodeOrToken::into_node)
    }

    fn tokens(self) -> impl Iterator<Item = Tok<'g>> + Clone {
        self.elements().filter_map(NodeOrToken::into_token)
    }

    fn node(self, kind: SyntaxKind) -> Option<Node<'g>> {
        self.nodes().find(|n| n.kind() == kind)
    }

    /// The first direct child token of `kind`.
    fn token(self, kind: SyntaxKind) -> Option<Tok<'g>> {
        self.tokens().find(|t| t.kind() == kind)
    }

    /// Range of the first non-trivia direct token (statements may hold leading comments).
    fn keyword_range(self) -> TextRange {
        self.tokens().find(|t| !t.kind().is_trivia()).map_or(self.range(), Tok::range)
    }
}

impl<'g> Tok<'g> {
    fn kind(self) -> SyntaxKind {
        CfxLua::kind_from_raw(self.data.kind())
    }

    fn text(self) -> &'g str {
        self.data.text()
    }

    fn range(self) -> TextRange {
        TextRange::at(self.offset, self.data.text_len())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Attr {
    None,
    Const,
    Close,
}

struct Local<'g> {
    /// `None` for hidden loop state.
    name: Option<Tok<'g>>,
    /// The implicit `self` of a method (it has no token).
    is_self: bool,
    attr: Attr,
}

impl Local<'_> {
    fn is_named(&self, name: &str) -> bool {
        self.name.map_or(self.is_self && name == "self", |n| n.text() == name)
    }
}

struct Func<'g> {
    vararg: bool,
    /// Active locals, outermost first.
    locals: Vec<Local<'g>>,
    /// Declared by the current `local` statement, active after it.
    pending: usize,
    limit_reported: bool,
    /// Index in `Validator::scopes` of this function's outermost block.
    first_scope: usize,
}

struct Goto<'g> {
    name: Tok<'g>,
    /// Active locals at the goto (or at the start of the block it left).
    level: usize,
}

struct Scope<'g> {
    start: usize,
    is_loop: bool,
    /// A `repeat` block whose statements ended; it closes after `until`.
    until_pending: bool,
    labels: Vec<Tok<'g>>,
    gotos: Vec<Goto<'g>>,
}

struct Validator<'g, 'e> {
    funcs: Vec<Func<'g>>,
    scopes: Vec<Scope<'g>>,
    /// Locals of the `local` statements being walked (one per function level).
    statements: Vec<Vec<Local<'g>>>,
    errors: &'e mut Vec<SyntaxError>,
}

impl<'g> Validator<'g, '_> {
    fn func(&mut self) -> &mut Func<'g> {
        self.funcs.last_mut().expect("main function")
    }

    fn func_ref(&self) -> &Func<'g> {
        self.funcs.last().expect("main function")
    }

    fn error(&mut self, range: TextRange, code: &'static str, message: String) {
        self.errors.push(SyntaxError { range, code, message });
    }

    /// `stack` holds the ancestors of `node`, each with its remaining children.
    fn enter(&mut self, node: Node<'g>, stack: &[(Node<'g>, Elements<'g>)]) {
        let parent = stack.last().map(|(n, _)| *n);
        match node.kind() {
            BLOCK => self.enter_block(parent),
            FUNC_BODY => self.enter_function(node, parent),
            LOCAL_FUNCTION_STAT => {
                if let Some(name) = node.token(NAME) {
                    self.declare(Some(name), Attr::None);
                }
            }
            LOCAL_STAT => self.enter_local(node),
            ASSIGN_STAT => {
                if let Some(list) = node.node(VAR_LIST) {
                    for target in list.nodes().filter(|n| n.kind() == NAME_EXPR) {
                        self.check_assign(target);
                    }
                }
                if let Some(t) = node.token(IN_KW) {
                    self.check_limit(t.range(), 1);
                }
            }
            COMPOUND_ASSIGN_STAT => {
                if let Some(target) = node.nodes().next().filter(|n| n.kind() == NAME_EXPR) {
                    self.check_assign(target);
                }
            }
            FUNCTION_STAT => {
                // `function x() end` assigns the variable `x`; `function t.x()` does not.
                if let Some(name) = node.node(FUNC_NAME) {
                    let plain = name.elements().all(|e| match e {
                        NodeOrToken::Token(t) => t.kind() == NAME || t.kind().is_trivia(),
                        NodeOrToken::Node(_) => false,
                    });
                    if plain {
                        self.check_assign(name);
                    }
                }
            }
            BREAK_STAT => {
                let first = self.func_ref().first_scope;
                if !self.scopes[first..].iter().any(|s| s.is_loop) {
                    self.error(node.keyword_range(), "BREAK_OUTSIDE", "'break' outside a loop".into());
                }
            }
            VARARG_EXPR => {
                if !self.func_ref().vararg {
                    let msg = "cannot use '...' outside a vararg function".into();
                    self.error(node.range(), "UNEXPECT_DOTS", msg);
                }
            }
            GOTO_STAT => self.goto(node),
            LABEL_STAT => self.label(node, stack),
            _ => {}
        }
    }

    fn leave(&mut self, node: Node<'g>, stack: &[(Node<'g>, Elements<'g>)]) {
        match node.kind() {
            BLOCK => {
                if stack.last().is_some_and(|(p, _)| p.kind() == REPEAT_STAT) {
                    // The `until` condition still sees the block's locals.
                    if let Some(scope) = self.scopes.last_mut() {
                        scope.until_pending = true;
                    }
                } else {
                    self.pop_scope();
                }
            }
            REPEAT_STAT => {
                if self.scopes.last().is_some_and(|s| s.until_pending) {
                    self.pop_scope();
                }
            }
            FUNC_BODY => {
                self.funcs.pop();
            }
            LOCAL_STAT => {
                let locals = self.statements.pop().unwrap_or_default();
                let func = self.func();
                func.pending -= locals.len();
                func.locals.extend(locals);
            }
            _ => {}
        }
    }

    fn enter_block(&mut self, parent: Option<Node<'g>>) {
        let parent_kind = parent.map(Node::kind);
        let is_loop = matches!(parent_kind, Some(WHILE_STAT | REPEAT_STAT | NUMERIC_FOR_STAT | GENERIC_FOR_STAT));
        let start = self.func_ref().locals.len();
        self.scopes.push(Scope { start, is_loop, until_pending: false, labels: Vec::new(), gotos: Vec::new() });
        let hidden = match parent_kind {
            Some(NUMERIC_FOR_STAT) => NUMERIC_FOR_HIDDEN,
            Some(GENERIC_FOR_STAT) => GENERIC_FOR_HIDDEN,
            _ => return,
        };
        for _ in 0..hidden {
            self.func().locals.push(Local { name: None, is_self: false, attr: Attr::None });
        }
        let stat = parent.expect("loop statement");
        for name in stat.tokens().filter(|t| t.kind() == NAME) {
            self.declare(Some(name), Attr::None);
        }
    }

    fn enter_function(&mut self, body: Node<'g>, parent: Option<Node<'g>>) {
        let params = body.node(PARAM_LIST);
        let params = params.into_iter().flat_map(|l| l.nodes()).filter(|n| n.kind() == PARAM);
        let vararg = params.clone().any(|p| p.token(DOT3).is_some());
        let first_scope = self.scopes.len();
        self.funcs.push(Func { vararg, locals: Vec::new(), pending: 0, limit_reported: false, first_scope });
        let is_method = parent
            .filter(|p| p.kind() == FUNCTION_STAT)
            .and_then(|p| p.node(FUNC_NAME))
            .is_some_and(|n| n.token(COLON).is_some());
        if is_method {
            self.func().locals.push(Local { name: None, is_self: true, attr: Attr::None });
        }
        for param in params {
            if param.token(DOT3).is_none()
                && let Some(name) = param.token(NAME)
            {
                self.declare(Some(name), Attr::None);
            }
        }
    }

    fn enter_local(&mut self, stat: Node<'g>) {
        let mut locals = Vec::new();
        let mut close_seen = false;
        for local in stat.nodes().filter(|n| n.kind() == LOCAL_NAME) {
            let attr = match local.node(ATTRIB) {
                Some(attrib) => self.attribute(attrib, &mut close_seen),
                None => Attr::None,
            };
            let name = local.token(NAME);
            self.check_limit(name.map_or(local.range(), Tok::range), 1);
            self.func().pending += 1;
            locals.push(Local { name, is_self: false, attr });
        }
        if let Some(t) = stat.token(IN_KW) {
            // lparser.c holds the table in a hidden `(in)` local while unpacking.
            self.check_limit(t.range(), 1);
        }
        self.statements.push(locals);
    }

    fn attribute(&mut self, attrib: Node<'g>, close_seen: &mut bool) -> Attr {
        let Some(name) = attrib.token(NAME) else { return Attr::None };
        match name.text() {
            "const" => Attr::Const,
            "close" => {
                if *close_seen {
                    let msg = "multiple to-be-closed variables in local list".into();
                    self.error(attrib.range(), "MULTI_CLOSE", msg);
                }
                *close_seen = true;
                Attr::Close
            }
            other => {
                let msg = format!("unknown attribute '{other}'");
                self.error(name.range(), "UNKNOWN_ATTRIBUTE", msg);
                Attr::None
            }
        }
    }

    /// Declare an active local in the current function.
    fn declare(&mut self, name: Option<Tok<'g>>, attr: Attr) {
        if let Some(n) = name {
            self.check_limit(n.range(), 1);
        }
        self.func().locals.push(Local { name, is_self: false, attr });
    }

    fn check_limit(&mut self, range: TextRange, extra: usize) {
        let func = self.func();
        if func.limit_reported || func.locals.len() + func.pending + extra <= MAX_LOCALS {
            return;
        }
        func.limit_reported = true;
        let msg = format!("too many local variables (limit is {MAX_LOCALS}) in function");
        self.error(range, "LOCAL_LIMIT", msg);
    }

    /// `target` is a NAME_EXPR or a plain FUNC_NAME.
    fn check_assign(&mut self, target: Node<'g>) {
        let Some(name) = target.token(NAME) else { return };
        let attr = self
            .funcs
            .iter()
            .rev()
            .flat_map(|f| f.locals.iter().rev())
            .find(|l| l.is_named(name.text()))
            .map_or(Attr::None, |l| l.attr);
        if attr != Attr::None {
            let msg = format!("attempt to assign to const variable '{}'", name.text());
            self.error(name.range(), "SET_CONST", msg);
        }
    }

    fn label_visible(&self, name: &str) -> bool {
        let first = self.func_ref().first_scope;
        self.scopes[first..].iter().any(|s| s.labels.iter().any(|l| l.text() == name))
    }

    fn goto(&mut self, stat: Node<'g>) {
        let Some(name) = stat.token(NAME) else { return };
        if self.label_visible(name.text()) {
            return; // backward jump: never enters a scope
        }
        let level = self.func_ref().locals.len();
        if let Some(scope) = self.scopes.last_mut() {
            scope.gotos.push(Goto { name, level });
        }
    }

    fn label(&mut self, stat: Node<'g>, stack: &[(Node<'g>, Elements<'g>)]) {
        let Some(name) = stat.token(NAME) else { return };
        if self.label_visible(name.text()) {
            let msg = format!("label '{}' already defined", name.text());
            self.error(name.range(), "REDEFINED_LABEL", msg);
        }
        // A label followed only by no-op statements up to `end` / `else` / `elseif` /
        // end of file is outside the scope of the block's locals (not before `until`).
        let after = stack.last().map(|(_, rest)| rest.clone());
        let only_noops = after.is_none_or(|rest| {
            rest.filter_map(NodeOrToken::into_node).all(|n| matches!(n.kind(), EMPTY_STAT | LABEL_STAT))
        });
        let in_repeat = stack.len() >= 2 && stack[stack.len() - 2].0.kind() == REPEAT_STAT;
        let locals = self.func_ref().locals.len();
        let Some(scope) = self.scopes.last_mut() else { return };
        let level = if only_noops && !in_repeat { scope.start } else { locals };
        scope.labels.push(name);
        let mut jumps = Vec::new();
        scope.gotos.retain(|g| {
            let solved = g.name.text() == name.text();
            if solved && g.level < level {
                jumps.push((g.name, g.level));
            }
            !solved
        });
        for (goto, at) in jumps {
            let local = self.func_ref().locals.get(at).and_then(|l| l.name).map_or("?", Tok::text);
            let msg = format!("goto '{}' jumps into the scope of local '{local}'", goto.text());
            self.error(goto.range(), "JUMP_LOCAL_SCOPE", msg);
        }
    }

    fn pop_scope(&mut self) {
        let Some(scope) = self.scopes.pop() else { return };
        let first = self.func_ref().first_scope;
        self.func().locals.truncate(scope.start);
        if self.scopes.len() > first {
            // Pending gotos move out to the enclosing block, at its level here.
            let parent = self.scopes.last_mut().expect("enclosing scope");
            parent.gotos.extend(scope.gotos.into_iter().map(|g| Goto { level: scope.start, ..g }));
        } else {
            for goto in scope.gotos {
                let msg = format!("no visible label '{}' for goto", goto.name.text());
                self.error(goto.name.range(), "NO_VISIBLE_LABEL", msg);
            }
        }
    }
}
