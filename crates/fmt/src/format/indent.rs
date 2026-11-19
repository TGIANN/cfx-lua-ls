//! `IndentationAnalyzer`: which nodes push an indent, and the indents that wait for
//! the walk (a new line, an indented previous sibling, a lazy break).

use super::{Fmt, IndentStrategy, Resolve};
use crate::style::IndentStyle;
use crate::tree::{Nk, Tk};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Standard,
    Invert,
    WhenNewLine,
    WhenPrevIndent,
    Keep,
}

struct Group {
    triggers: Vec<usize>,
    indent: usize,
    parent: usize,
}

pub(crate) struct State {
    indent: Vec<Option<(Kind, usize)>>,
    mark: Vec<bool>,
    wait: Vec<Option<usize>>,
    groups: Vec<Group>,
}

impl State {
    pub(super) fn new(n: usize) -> State {
        State { indent: vec![None; n], mark: vec![false; n], wait: vec![None; n], groups: Vec::new() }
    }
}

impl Fmt<'_, '_> {
    pub(super) fn indent_analyze(&mut self) {
        let t = self.t;
        let s = self.style;
        let cont = s.continuation_indent;
        for n in t.all() {
            let Some(kind) = t.node_kind(n) else { continue };
            match kind {
                Nk::Block => {
                    self.add_indenter(n, Kind::Standard, 0);
                    if s.never_indent_comment_on_if_branch {
                        self.never_indent_comment_on_if_branch(n);
                    }
                    if s.allow_non_indented_comments {
                        for c in t.children(n) {
                            if matches!(t.token_kind(c), Some(Tk::ShortComment | Tk::LongComment)) && t.start_col(c) == 0 {
                                self.add_indenter(c, Kind::Keep, 0);
                            }
                        }
                    }
                }
                Nk::ParamList => self.add_indenter(n, Kind::Standard, cont.before_block),
                Nk::CallExpression => {
                    if t.is_token(t.child_token(n, Tk::LParen)) {
                        let list = t.child_node(n, Nk::ExpressionList);
                        if t.is_node(list) {
                            let indent = t.child_exprs(list).into_iter().any(|e| self.expr_should_indent(e));
                            if indent {
                                self.add_indenter(list, Kind::Standard, 0);
                            } else {
                                self.add_linebreak_group(list, 0);
                            }
                        }
                    }
                }
                Nk::ParExpression | Nk::LocalStatement | Nk::AssignStatement | Nk::ReturnStatement => {
                    if kind == Nk::ParExpression {
                        let expr = t.child_expr(n);
                        if t.is_node(expr) && self.expr_should_indent(expr) {
                            self.add_indenter(expr, Kind::Standard, 0);
                        } else {
                            self.add_indenter(expr, Kind::WhenNewLine, 0);
                        }
                    }
                    // C++ falls through from the parenthesised expression; it has no list.
                    let list = t.child_node(n, Nk::ExpressionList);
                    if t.is_node(list) {
                        self.indent_expr_list(list);
                    }
                }
                Nk::WhileStatement | Nk::RepeatStatement => {
                    let expr = t.child_expr(n);
                    if t.is_node(expr) {
                        self.add_indenter(expr, Kind::Standard, 0);
                    }
                }
                Nk::IfStatement => {
                    if !s.never_indent_before_if_condition {
                        for expr in t.child_exprs(n) {
                            self.add_indenter(expr, Kind::Standard, cont.before_block);
                        }
                    }
                }
                Nk::ExpressionStatement => {
                    let suffixed = t.child_node(n, Nk::SuffixedExpression);
                    self.indent_suffixed(suffixed, cont.in_expr);
                }
                Nk::TableExpression => {
                    let list = t.child_node(n, Nk::TableFieldList);
                    let unit = match s.indent_style {
                        IndentStyle::Space => s.indent_size,
                        IndentStyle::Tab => s.tab_width,
                    };
                    for field in t.children(list) {
                        self.add_indenter(field, Kind::WhenNewLine, unit);
                    }
                }
                Nk::TableField => {
                    if t.is_token(t.child_token(n, Tk::Assign)) {
                        let expr = t.last_child_expr(n);
                        let k = t.node_kind(expr);
                        if k != Some(Nk::TableExpression)
                            && k != Some(Nk::StringLiteralExpression)
                            && self.expr_should_indent(expr)
                        {
                            self.add_indenter(expr, Kind::Standard, cont.in_expr);
                        }
                    } else {
                        let suffixed = t.child_node(n, Nk::SuffixedExpression);
                        self.indent_suffixed(suffixed, cont.in_table);
                    }
                }
                _ => {}
            }
        }
    }

    pub(super) fn indent_query(&mut self, n: usize, r: &mut Resolve) {
        let Some((kind, indent)) = self.indent.indent[n] else { return };
        match kind {
            Kind::Standard => r.set_indent(indent, IndentStrategy::Relative),
            Kind::Invert => r.set_indent(indent, IndentStrategy::InvertRelative),
            Kind::WhenNewLine => {
                if self.is_new_line_at(n) {
                    r.set_indent(indent, IndentStrategy::Relative);
                }
            }
            Kind::WhenPrevIndent => {
                if self.indent.mark[self.t.prev_sibling(n)] {
                    r.set_indent(indent, IndentStrategy::Relative);
                }
            }
            Kind::Keep => r.set_indent(0, IndentStrategy::Absolute),
        }
    }

    /// `MarkIndent`: a token starts a line; it and its parents up to the statement
    /// count as indented.
    pub(super) fn indent_mark(&mut self, n: usize) {
        let t = self.t;
        self.indent.mark[n] = true;
        let mut p = t.parent(n);
        while p != 0 && !t.node_kind(p).is_some_and(|k| k == Nk::Block || k.is_stmt()) {
            self.indent.mark[p] = true;
            p = t.parent(p);
        }
    }

    /// `ProcessExceedLinebreak`: a lazy break happened before `n`; its group indents
    /// on every later new line.
    pub(super) fn indent_exceed_linebreak(&mut self, n: usize) {
        let Some(pos) = self.indent.wait[n] else { return };
        let Some(group) = self.indent.groups.get(pos) else { return };
        let (indent, parent) = (group.indent, group.parent);
        for i in 0..self.indent.groups[pos].triggers.len() {
            let trigger = self.indent.groups[pos].triggers[i];
            self.indent.wait[trigger] = None;
        }
        for c in self.t.children(parent) {
            self.add_indenter(c, Kind::WhenNewLine, indent);
        }
    }

    fn add_indenter(&mut self, n: usize, kind: Kind, indent: usize) {
        self.indent.indent[n] = Some((kind, indent));
    }

    fn add_linebreak_group(&mut self, parent: usize, indent: usize) {
        let pos = self.indent.groups.len();
        let triggers: Vec<usize> = self.t.children(parent).collect();
        for &n in &triggers {
            // `unordered_map::insert` keeps an existing entry.
            self.indent.wait[n].get_or_insert(pos);
        }
        self.indent.groups.push(Group { triggers, indent, parent });
    }

    fn indent_suffixed(&mut self, suffixed: usize, indent: usize) {
        let t = self.t;
        for expr in t.children(suffixed) {
            match t.node_kind(expr) {
                Some(Nk::IndexExpression) => self.add_indenter(expr, Kind::Standard, indent),
                Some(Nk::CallExpression) if t.node_kind(t.prev_sibling(expr)) != Some(Nk::NameExpression) => {
                    self.add_indenter(expr, Kind::WhenPrevIndent, indent);
                }
                _ => {}
            }
        }
    }

    fn indent_expr_list(&mut self, list: usize) {
        let t = self.t;
        let exprs = t.child_exprs(list);
        let indent = if let [expr] = exprs[..] {
            if matches!(t.node_kind(expr), Some(Nk::TableExpression | Nk::StringLiteralExpression)) {
                return;
            }
            self.expr_should_indent(expr)
        } else {
            exprs.into_iter().any(|e| self.expr_should_indent(e))
        };
        let in_expr = self.style.continuation_indent.in_expr;
        if indent {
            self.add_indenter(list, Kind::Standard, in_expr);
        } else {
            self.add_linebreak_group(list, in_expr);
        }
    }

    /// `IsExprShouldIndent`: the expression starts on a later line than the token
    /// before it, or a part of it does. Walked with a stack, not recursion.
    fn expr_should_indent(&self, expr: usize) -> bool {
        let t = self.t;
        let starts_late = |n: usize| t.start_line(n) != t.end_line(t.prev_token(n));
        let mut stack = vec![expr];
        while let Some(e) = stack.pop() {
            if starts_late(e) {
                return true;
            }
            if t.is_single_line(e) {
                continue;
            }
            match t.node_kind(e) {
                Some(Nk::SuffixedExpression) => stack.extend(t.child_exprs(e)),
                Some(Nk::UnaryExpression) => {
                    let sub = t.child_expr(e);
                    if t.is_node(sub) {
                        stack.push(sub);
                    }
                }
                Some(Nk::BinaryExpression) => {
                    for c in t.children(e) {
                        if t.is_node(c) {
                            stack.push(c);
                        } else if t.token_kind(c).is_some_and(Tk::is_binary_op) && starts_late(c) {
                            return true;
                        }
                    }
                }
                _ => {}
            }
        }
        false
    }

    fn never_indent_comment_on_if_branch(&mut self, block: usize) {
        let t = self.t;
        if t.node_kind(t.parent(block)) != Some(Nk::IfStatement) {
            return;
        }
        let branch = t.next_token(block);
        if !matches!(t.token_kind(branch), Some(Tk::Elseif | Tk::Else)) {
            return;
        }
        let children: Vec<usize> = t.children(block).collect();
        if !children.iter().any(|&c| t.is_node(c)) {
            return;
        }
        let mut sibling_line = t.start_line(branch);
        for &n in children.iter().rev() {
            if t.token_kind(n) != Some(Tk::ShortComment) {
                break;
            }
            let line = t.start_line(n);
            if line + 1 == sibling_line {
                self.add_indenter(n, Kind::Invert, 0);
                sibling_line = line;
            }
        }
    }
}
