//! `LineBreakAnalyzer`: forced breaks after statements and comments, and lazy
//! breaks that only happen when the line would pass `max_line_length`.

use super::{Fmt, NextSpace, Resolve};
use crate::style::{BreakTableList, LineSpace, LineSpaceType};
use crate::tree::{Nk, Tk};

#[derive(Debug, Clone, Copy)]
enum Break {
    Standard(LineSpace),
    /// Break before the related node when it would pass the line limit.
    WhenMayExceed(usize),
    Never,
}

pub(crate) struct State {
    breaks: Vec<Option<Break>>,
}

impl State {
    pub(super) fn new(n: usize) -> State {
        State { breaks: vec![None; n] }
    }
}

/// The recursive `AnalyzeExpr` calls, kept on an explicit stack: a long chain of
/// binary operators nests without bound.
enum Task {
    Expr(usize),
    Lazy(usize),
}

impl Fmt<'_, '_> {
    pub(super) fn line_break_analyze(&mut self) {
        let t = self.t;
        for n in t.all() {
            match t.token_kind(n) {
                Some(Tk::Shebang | Tk::ShortComment) => self.break_after(n, LineSpace::keep()),
                Some(Tk::LCurly | Tk::RCurly) if self.style.break_before_braces => {
                    let parent = t.parent(n);
                    if t.node_kind(parent) == Some(Nk::TableExpression) && !t.is_single_line(parent) {
                        self.break_before(n, LineSpace::keep());
                    }
                }
                _ => {}
            }
        }
    }

    pub(super) fn line_break_complex_analyze(&mut self) {
        let t = self.t;
        let s = self.style;
        for n in t.all() {
            let Some(kind) = t.node_kind(n) else { continue };
            match kind {
                Nk::Block => {
                    let children: Vec<usize> = t.children(n).collect();
                    if t.is_single_line(t.parent(n)) {
                        if children.len() <= 1 {
                            self.space_around(n, 1);
                            continue;
                        }
                    } else if children.is_empty() {
                        let end = t.child_token(t.parent(n), Tk::End);
                        let prev = t.prev_token(end);
                        self.break_after(prev, LineSpace::new(LineSpaceType::Max, 2));
                        continue;
                    }
                    for &stmt in &children {
                        if let Some(k) = t.node_kind(stmt) {
                            let line = match k {
                                Nk::LocalStatement | Nk::AssignStatement => s.line_space_after_local_or_assign_statement,
                                Nk::IfStatement => s.line_space_after_if_statement,
                                Nk::DoStatement => s.line_space_after_do_statement,
                                Nk::WhileStatement => s.line_space_after_while_statement,
                                Nk::RepeatStatement => s.line_space_after_repeat_statement,
                                Nk::ForStatement => s.line_space_after_for_statement,
                                Nk::FunctionStatement => s.line_space_after_function_statement,
                                Nk::ExpressionStatement => s.line_space_after_expression_statement,
                                _ => LineSpace::keep(),
                            };
                            self.break_after(stmt, line);
                            if t.is_token(t.child_token(stmt, Tk::Semi)) {
                                let next = t.next_sibling(stmt);
                                if t.start_line(next) == t.end_line(stmt) {
                                    self.cancel_break_after(stmt);
                                }
                            }
                        } else {
                            match t.token_kind(stmt) {
                                // C++ falls through from the long comment into the short one.
                                Some(Tk::LongComment) if t.token_kind(t.next_token(stmt)) == Some(Tk::ShortComment) => {}
                                Some(Tk::LongComment | Tk::ShortComment | Tk::Shebang) => {
                                    self.break_after(stmt, s.line_space_after_comment);
                                }
                                _ => self.break_after(stmt, LineSpace::fixed(1)),
                            }
                        }
                    }
                    self.break_before(n, s.line_space_around_block);
                    self.break_after(n, s.line_space_around_block);
                }
                Nk::LocalStatement | Nk::AssignStatement | Nk::ReturnStatement => {
                    let list = t.child_node(n, Nk::ExpressionList);
                    if t.is_node(list) {
                        self.analyze_expr_list(list);
                    }
                }
                // `AnalyzeConditionExpr` does nothing, so `if`, `while` and `repeat`
                // have nothing to mark.
                Nk::ForStatement => {
                    let names = t.child_node(n, Nk::NameDefList);
                    if t.is_node(names) {
                        self.analyze_name_list(names);
                    }
                    let list = t.child_node(n, Nk::ExpressionList);
                    if t.is_node(list) {
                        self.analyze_expr_list(list);
                    }
                }
                Nk::FunctionBody => {
                    let params = t.child_node(n, Nk::ParamList);
                    if t.is_node(params) {
                        self.analyze_name_list(params);
                    }
                }
                Nk::ExpressionStatement => {
                    let suffixed = t.child_node(n, Nk::SuffixedExpression);
                    if t.is_node(suffixed) {
                        self.analyze_exprs(vec![Task::Expr(suffixed)]);
                    }
                }
                _ => {}
            }
        }
    }

    pub(super) fn line_break_query(&mut self, n: usize, r: &mut Resolve) {
        let t = self.t;
        let data = self.line_break.breaks.get(n).copied().flatten();
        let collapse = matches!(data, Some(Break::Never));
        if t.is_token(n) && !collapse && matches!(r.next_space, NextSpace::None | NextSpace::Space(_)) {
            let next = t.next_token(n);
            if t.is_token(next) {
                let (current, next_line) = (t.end_line(n), t.start_line(next));
                if next_line > current {
                    r.next_space = NextSpace::LineBreak(LineSpace::fixed(next_line - current));
                }
            }
        }
        match data {
            Some(Break::Standard(line)) => r.next_space = NextSpace::LineBreak(line),
            Some(Break::WhenMayExceed(related)) => {
                let guess = self.width + t.first_line_width(n) + t.first_line_width(related);
                if guess > self.style.max_line_length {
                    self.indent_exceed_linebreak(n);
                    r.next_space = NextSpace::LineBreak(LineSpace::fixed(1));
                }
            }
            Some(Break::Never) | None => {}
        }
    }

    pub(super) fn break_after(&mut self, n: usize, line: LineSpace) {
        let token = self.t.last_token(n);
        if self.t.is_token(token) {
            self.line_break.breaks[token] = Some(Break::Standard(line));
        }
    }

    pub(super) fn break_before(&mut self, n: usize, line: LineSpace) {
        let prev = self.t.prev_token(n);
        if prev != 0 {
            self.break_after(prev, line);
        }
    }

    pub(super) fn mark_lazy_break(&mut self, n: usize) {
        let prev = self.t.prev_token(n);
        if self.t.is_token(prev) && self.line_break.breaks[prev].is_none() {
            self.line_break.breaks[prev] = Some(Break::WhenMayExceed(n));
        }
    }

    pub(super) fn mark_not_break(&mut self, n: usize) {
        let prev = self.t.prev_token(n);
        if self.t.is_token(prev) && self.line_break.breaks[prev].is_none() {
            self.line_break.breaks[prev] = Some(Break::Never);
        }
    }

    pub(super) fn cancel_break_after(&mut self, n: usize) {
        let token = self.t.last_token(n);
        if self.t.is_token(token) {
            self.line_break.breaks[token] = Some(Break::Never);
        }
    }

    fn analyze_expr_list(&mut self, list: usize) {
        let mut stack = Vec::new();
        self.push_expr_list(list, &mut stack);
        self.analyze_exprs(stack);
    }

    /// `AnalyzeExpr`, `AnalyzeSuffixedExpr` and what they call, in C++'s order.
    fn analyze_exprs(&mut self, mut stack: Vec<Task>) {
        let t = self.t;
        while let Some(task) = stack.pop() {
            let expr = match task {
                Task::Lazy(n) => {
                    self.mark_lazy_break(n);
                    continue;
                }
                Task::Expr(n) => n,
            };
            match t.node_kind(expr) {
                Some(Nk::BinaryExpression) => {
                    if let [left, right] = t.child_exprs(expr)[..] {
                        stack.push(Task::Expr(right));
                        stack.push(Task::Lazy(right));
                        stack.push(Task::Expr(left));
                    }
                }
                Some(Nk::ClosureExpression) => {
                    let params = t.child_node(expr, Nk::ParamList);
                    self.analyze_name_list(params);
                }
                Some(Nk::UnaryExpression) => stack.push(Task::Expr(t.child_expr(expr))),
                Some(Nk::TableExpression) => self.analyze_table(expr, &mut stack),
                Some(Nk::CallExpression) => {
                    let l = t.child_node(expr, Nk::ExpressionList);
                    self.push_expr_list(l, &mut stack);
                }
                Some(Nk::StringLiteralExpression | Nk::NameExpression | Nk::IndexExpression) => {
                    self.mark_lazy_break(expr);
                }
                Some(Nk::SuffixedExpression) => {
                    let children: Vec<usize> = t.children(expr).collect();
                    stack.extend(children.into_iter().rev().map(Task::Expr));
                }
                _ => {}
            }
        }
    }

    fn push_expr_list(&self, list: usize, stack: &mut Vec<Task>) {
        let exprs = self.t.child_exprs(list);
        if exprs.len() == 1 {
            stack.push(Task::Expr(exprs[0]));
        } else {
            for &e in exprs.iter().rev() {
                stack.push(Task::Expr(e));
                stack.push(Task::Lazy(e));
            }
        }
    }

    fn analyze_name_list(&mut self, list: usize) {
        let t = self.t;
        let names = t.child_tokens(list, Tk::Name);
        if self.style.auto_collapse_lines && self.can_collapse_lines(list) {
            for name in names {
                self.mark_not_break(name);
            }
            return;
        }
        if self.style.break_all_list_when_line_exceed && self.can_break_all(list) {
            for name in names {
                self.break_before(name, LineSpace::fixed(1));
            }
            return;
        }
        for name in names {
            self.mark_lazy_break(name);
        }
    }

    fn analyze_table(&mut self, table: usize, stack: &mut Vec<Task>) {
        let t = self.t;
        let s = self.style;
        let list = t.child_node(table, Nk::TableFieldList);
        let fields = t.child_nodes(list, Nk::TableField);

        if s.auto_collapse_lines && self.can_collapse_lines(list) {
            for &field in &fields {
                self.mark_not_break(field);
            }
            self.mark_not_break(t.next_token(list));
            return;
        }
        if s.break_table_list == BreakTableList::Never {
            return;
        }
        let mut break_all = s.break_all_list_when_line_exceed && self.can_break_all(list);
        if !break_all && s.break_table_list == BreakTableList::Smart {
            break_all = t.start_line(list) != t.end_line(list)
                && fields.iter().any(|&f| t.is_token(t.child_token(f, Tk::Assign)));
        }
        if break_all {
            let lcurly = t.child_token(table, Tk::LCurly);
            let after = t.next_sibling(lcurly);
            if matches!(t.token_kind(after), Some(Tk::ShortComment | Tk::LongComment)) {
                self.break_after(after, LineSpace::keep());
            } else {
                self.break_after(lcurly, LineSpace::keep());
            }
            for &field in &fields {
                self.break_after(field, LineSpace::keep());
            }
        }
        // A one-line table inside a table stays as it is: `{ {}, {} }`.
        if s.break_table_list == BreakTableList::Smart
            && t.node_kind(t.parent(t.parent(table))) == Some(Nk::TableFieldList)
            && t.is_single_line(table)
        {
            return;
        }
        let exprs: Vec<usize> = fields.iter().flat_map(|&f| t.child_exprs(f)).collect();
        stack.extend(exprs.into_iter().rev().map(Task::Expr));
    }

    fn can_break_all(&self, n: usize) -> bool {
        let t = self.t;
        match t.node_kind(n) {
            Some(Nk::ParamList | Nk::TableFieldList) => {
                t.end_col(n) >= self.style.max_line_length || !t.is_single_line(n)
            }
            _ => false,
        }
    }

    fn can_collapse_lines(&self, n: usize) -> bool {
        let t = self.t;
        let prev = t.prev_token(n);
        if prev == 0 {
            return false;
        }
        match t.node_kind(n) {
            Some(Nk::ParamList | Nk::TableFieldList) => {
                let mut width = t.end_col(prev);
                for child in t.children(n) {
                    match t.token_kind(child) {
                        Some(Tk::Comma) => width += 1,
                        Some(Tk::ShortComment | Tk::LongComment) => return false,
                        _ => {}
                    }
                    width += t.node_text(child).len();
                }
                width <= self.style.max_line_length
            }
            _ => false,
        }
    }
}
