//! `AlignAnalyzer`: continuous `=` columns, call and parameter lists aligned to
//! their first element, array tables, `if` conditions, chains, inline comments.

use super::{Fmt, IndentStrategy, PrevSpace, Resolve};
use crate::style::{AlignArrayTable, AlignChainExpr, ContinuousAlign, SpaceAroundStyle, SpaceBeforeInlineCommentStyle};
use crate::tree::{Nk, Tk, Tree};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Strategy {
    Normal,
    EqWhenExtraSpace,
    EqAlways,
    ToFirst,
    Comment,
}

struct Group {
    strategy: Strategy,
    nodes: Vec<usize>,
    resolved: bool,
    pos: usize,
}

pub(crate) struct State {
    groups: Vec<Group>,
    start: Vec<Option<usize>>,
    resolve: Vec<Option<usize>>,
    inline_comments: Vec<Vec<usize>>,
}

impl State {
    pub(super) fn new(n: usize) -> State {
        State { groups: Vec::new(), start: vec![None; n], resolve: vec![None; n], inline_comments: Vec::new() }
    }
}

/// `TextRange::GetEndOffset`: inclusive, the start for an empty range.
fn range_end(t: &Tree, n: usize) -> usize {
    let (start, len) = t.text_range(n);
    if len == 0 { start } else { start + len - 1 }
}

impl Fmt<'_, '_> {
    pub(super) fn align_analyze(&mut self) {
        let t = self.t;
        let s = self.style;
        for n in t.all() {
            if let Some(kind) = t.node_kind(n) {
                match kind {
                    Nk::Block => {
                        if s.align_continuous_assign_statement != ContinuousAlign::None {
                            self.align_continuous_local_or_assign(n);
                        }
                        if s.align_continuous_similar_call_args {
                            self.align_continuous_similar_call_args(n);
                        }
                    }
                    Nk::TableFieldList => {
                        if s.align_continuous_rect_table_field != ContinuousAlign::None {
                            self.align_continuous_rect_field(n);
                        }
                        if s.align_array_table != AlignArrayTable::None {
                            self.align_continuous_array_table_field(n);
                        }
                    }
                    Nk::CallExpression => {
                        if s.align_call_args {
                            let list = t.child_node(n, Nk::ExpressionList);
                            let exprs = t.child_exprs(list);
                            let symbol_line = t.end_line(t.prev_token(list));
                            if !exprs.iter().all(|&e| t.start_line(e) == symbol_line) {
                                self.push_group(Strategy::ToFirst, exprs);
                            }
                        }
                    }
                    Nk::ParamList => {
                        if s.align_function_params {
                            let group = t
                                .children(n)
                                .filter(|&c| matches!(t.token_kind(c), Some(Tk::Name | Tk::Dots)))
                                .collect();
                            self.push_group(Strategy::ToFirst, group);
                        }
                    }
                    // C++ falls through from the `if` statement into the chain case.
                    Nk::IfStatement | Nk::SuffixedExpression => {
                        if kind == Nk::IfStatement && s.align_if_branch {
                            self.align_if_statement(n);
                        }
                        if s.align_chain_expr != AlignChainExpr::None {
                            self.align_chain_expr(n);
                        }
                    }
                    _ => {}
                }
            } else if t.token_kind(n) == Some(Tk::ShortComment)
                && s.align_continuous_inline_comment
                && s.space_before_inline_comment.style == SpaceBeforeInlineCommentStyle::Fixed
            {
                self.align_inline_comment(n);
            }
        }
        for group in std::mem::take(&mut self.align.inline_comments) {
            self.push_group(Strategy::Comment, group);
        }
    }

    pub(super) fn align_query(&mut self, n: usize, r: &mut Resolve) {
        if let Some(g) = self.align.start[n]
            && !self.align.groups[g].resolved
        {
            self.resolve_align_group(g);
            self.align.groups[g].resolved = true;
        }
        if let Some(g) = self.align.resolve[n] {
            let group = &self.align.groups[g];
            match group.strategy {
                Strategy::Normal | Strategy::EqWhenExtraSpace | Strategy::EqAlways => {
                    r.prev_space = PrevSpace::AlignRelativeIndent(group.pos);
                }
                Strategy::ToFirst => r.set_indent(group.pos, IndentStrategy::Absolute),
                Strategy::Comment => r.prev_space = PrevSpace::AlignPos(group.pos),
            }
        }
    }

    fn push_group(&mut self, strategy: Strategy, nodes: Vec<usize>) {
        let pos = self.align.groups.len();
        if let Some(&first) = nodes.first() {
            self.align.start[first] = Some(pos);
        }
        self.align.groups.push(Group { strategy, nodes, resolved: false, pos: 0 });
    }

    fn push_normal_group(&mut self, align_pos: usize, nodes: Vec<usize>) {
        let pos = self.align.groups.len();
        for &n in &nodes {
            self.align.resolve[n] = Some(pos);
        }
        self.align.groups.push(Group { strategy: Strategy::Normal, nodes, resolved: true, pos: align_pos });
    }

    fn eq_strategy(setting: ContinuousAlign) -> Strategy {
        if setting == ContinuousAlign::Always { Strategy::EqAlways } else { Strategy::EqWhenExtraSpace }
    }

    fn align_continuous_local_or_assign(&mut self, block: usize) {
        let t = self.t;
        let strategy = Self::eq_strategy(self.style.align_continuous_assign_statement);
        let space = self.style.align_continuous_line_space;
        let is_assign = |k: Option<Nk>| matches!(k, Some(Nk::LocalStatement | Nk::AssignStatement));
        let mut last_line = 0;
        let mut group: Vec<usize> = Vec::new();
        for stmt in t.children(block) {
            let kind = t.node_kind(stmt);
            if group.is_empty() {
                if is_assign(kind) {
                    group.push(stmt);
                    last_line = t.end_line(stmt);
                }
                continue;
            }
            if t.token_kind(stmt) == Some(Tk::ShortComment) {
                if t.start_line(stmt).wrapping_sub(last_line) > space {
                    if group.len() > 1 {
                        self.push_group(strategy, std::mem::take(&mut group));
                    }
                    group.clear();
                } else {
                    last_line = t.end_line(stmt);
                }
                continue;
            }
            if is_assign(kind) {
                if t.start_line(stmt).wrapping_sub(last_line) <= space {
                    group.push(stmt);
                } else {
                    if group.len() > 1 {
                        self.push_group(strategy, std::mem::take(&mut group));
                    }
                    group.clear();
                    group.push(stmt);
                }
                last_line = t.end_line(stmt);
            } else {
                if group.len() > 1 {
                    self.push_group(strategy, std::mem::take(&mut group));
                }
                group.clear();
            }
        }
        if group.len() > 1 {
            self.push_group(strategy, group);
        }
    }

    fn align_continuous_rect_field(&mut self, list: usize) {
        let t = self.t;
        let strategy = Self::eq_strategy(self.style.align_continuous_rect_table_field);
        let space = self.style.align_continuous_line_space;
        let is_rect = |n: usize| t.node_kind(n) == Some(Nk::TableField) && t.child_token(n, Tk::Assign) != 0;
        let mut last_line = 0;
        let mut group: Vec<usize> = Vec::new();
        for field in t.children(list) {
            let line = t.start_line(field);
            if line < last_line {
                continue;
            }
            if group.is_empty() {
                if is_rect(field) {
                    group.push(field);
                    last_line = t.end_line(field);
                }
                continue;
            }
            if let Some(tk) = t.token_kind(field) {
                if tk == Tk::ShortComment {
                    if line.wrapping_sub(last_line) > space {
                        if group.len() > 1 {
                            self.push_group(strategy, std::mem::take(&mut group));
                        }
                        group.clear();
                    } else {
                        last_line = t.end_line(field);
                    }
                }
                continue;
            }
            if is_rect(field) {
                if line == last_line {
                    group.clear();
                    last_line += 1;
                    continue;
                } else if line - last_line <= space {
                    group.push(field);
                } else {
                    if group.len() > 1 {
                        self.push_group(strategy, std::mem::take(&mut group));
                    }
                    group.clear();
                    group.push(field);
                }
                last_line = t.end_line(field);
            } else {
                if group.len() > 1 {
                    self.push_group(strategy, std::mem::take(&mut group));
                }
                group.clear();
            }
        }
        if group.len() > 1 {
            self.push_group(strategy, group);
        }
    }

    fn align_continuous_array_table_field(&mut self, list: usize) {
        let t = self.t;
        let mut tables: Vec<usize> = Vec::new();
        let mut last_line = 0;
        for field in t.children(list) {
            if matches!(t.token_kind(field), Some(Tk::ShortComment | Tk::LongComment)) {
                continue;
            }
            let array = t.node_kind(field) == Some(Nk::TableField)
                && t.node_kind(t.first_child(field)) == Some(Nk::TableExpression);
            if array && t.is_single_line(field) && t.start_line(field) > last_line {
                last_line = t.start_line(field);
                tables.push(t.first_child(field));
            } else if tables.len() > 1 {
                self.align_array_tables(std::mem::take(&mut tables));
            }
        }
        if tables.len() > 1 {
            self.align_array_tables(tables);
        }
    }

    fn align_array_tables(&mut self, tables: Vec<usize>) {
        let t = self.t;
        let s = self.style;
        let mut fields_of: Vec<Vec<usize>> = Vec::new();
        let mut max_align = 0;
        for &table in &tables {
            if t.non_uniform_char_before(range_end(t, table)) {
                return;
            }
            let list = t.child_node(table, Nk::TableFieldList);
            let fields = t.child_nodes(list, Nk::TableField);
            max_align = max_align.max(fields.len());
            fields_of.push(fields);
        }
        let mut align_pos = 1 + s.space_around_table_field_list as usize;
        for i in 0..max_align {
            let mut element = 0;
            let mut group = Vec::new();
            for fields in &fields_of {
                if let Some(&field) = fields.get(i) {
                    element = element.max(t.utf8_len(field));
                    group.push(t.first_token(field));
                }
            }
            self.push_normal_group(align_pos, group);
            align_pos += element;
            if i + 1 == max_align {
                align_pos += s.space_around_table_field_list as usize;
            } else {
                align_pos += s.space_after_comma as usize;
            }
        }
        if s.align_array_table == AlignArrayTable::ContainCurly {
            let group = tables.iter().map(|&table| t.child_token(table, Tk::RCurly)).collect();
            self.push_normal_group(align_pos, group);
        }
    }

    fn resolve_align_group(&mut self, g: usize) {
        let t = self.t;
        let s = self.style;
        let prev_space = if s.space_around_assign_operator == SpaceAroundStyle::Always { 2 } else { 1 };
        let strategy = self.align.groups[g].strategy;
        let nodes = std::mem::take(&mut self.align.groups[g].nodes);
        match strategy {
            Strategy::EqWhenExtraSpace => {
                let mut allow = false;
                for &n in &nodes {
                    let eq = t.child_token(n, Tk::Assign);
                    if t.is_token(eq) {
                        let diff = t.text_range(eq).0.wrapping_sub(range_end(t, t.prev_token(eq)));
                        if diff > 2 {
                            allow = true;
                        }
                        if t.non_uniform_char_before(t.text_range(eq).0) {
                            self.align.groups[g].nodes = nodes;
                            return;
                        }
                    }
                }
                if allow {
                    let mut max = 0;
                    for &n in &nodes {
                        let eq = t.child_token(n, Tk::Assign);
                        if t.is_token(eq) {
                            let prev = t.prev_token(eq);
                            max = max.max((t.end_col(prev) + prev_space).saturating_sub(t.start_col(n)));
                            self.align.resolve[eq] = Some(g);
                        }
                    }
                    self.align.groups[g].pos = max;
                }
            }
            Strategy::EqAlways => {
                let mut max = 0;
                for &n in &nodes {
                    let eq = t.child_token(n, Tk::Assign);
                    if t.is_token(eq) {
                        if t.non_uniform_char_before(t.text_range(eq).0) {
                            self.align.groups[g].nodes = nodes;
                            return;
                        }
                        let prev = t.prev_token(eq);
                        max = max.max((range_end(t, prev) + prev_space).saturating_sub(t.text_range(n).0));
                        self.align.resolve[eq] = Some(g);
                    }
                }
                self.align.groups[g].pos = max;
            }
            Strategy::ToFirst => {
                if !nodes.is_empty() && !self.is_new_line_at(nodes[0]) {
                    // Diagnostic mode writes nothing: the source column stands in.
                    self.align.groups[g].pos = if self.diagnostic { t.start_col(nodes[0]) } else { self.width };
                    for &n in &nodes {
                        self.align.resolve[n] = Some(g);
                    }
                }
            }
            Strategy::Comment => {
                let mut max = 0;
                for &c in &nodes {
                    if t.is_token(c) {
                        if t.non_uniform_char_before(t.text_range(c).0) {
                            self.align.groups[g].nodes = nodes;
                            return;
                        }
                        let prev = t.prev_token(c);
                        max = max.max(t.column(range_end(t, prev)) + s.space_before_inline_comment.space + 1);
                        self.align.resolve[c] = Some(g);
                    }
                }
                self.align.groups[g].pos = max;
            }
            Strategy::Normal => {}
        }
        self.align.groups[g].nodes = nodes;
    }

    fn align_if_statement(&mut self, n: usize) {
        let t = self.t;
        let if_ = t.child_token(n, Tk::If);
        let elseifs = t.child_tokens(n, Tk::Elseif);
        let mut group = vec![t.next_token(if_)];
        let condition = t.next_sibling(if_);
        let mut logic_ops: Vec<usize> = Vec::new();
        let mut if_align = 0;
        if t.node_kind(condition) == Some(Nk::BinaryExpression) {
            let ops = self.collect_logic_ops(condition);
            for &op in &ops {
                if t.end_line(t.prev_token(op)) != t.start_line(op) {
                    match t.token_kind(op) {
                        Some(Tk::And) => if_align = if_align.max(4),
                        Some(Tk::Or) => if_align = if_align.max(3),
                        _ => {}
                    }
                }
            }
            logic_ops = ops;
        }
        if elseifs.is_empty() {
            if if_align != 0 {
                group.extend(logic_ops.iter().map(|&op| t.next_token(op)).filter(|&n| t.is_token(n)));
                self.push_normal_group(if_align, group);
            }
            return;
        }
        let space_after_if = t.start_col(t.next_token(if_)).wrapping_sub(t.start_col(if_));
        if space_after_if == 3 && if_align == 0 {
            group.clear();
            logic_ops.clear();
        }
        for elseif in elseifs {
            logic_ops.extend(self.collect_logic_ops(t.next_sibling(elseif)));
        }
        group.extend(logic_ops.iter().map(|&op| t.next_token(op)).filter(|&n| t.is_token(n)));
        self.push_normal_group(7, group);
    }

    /// `helper::CollectBinaryOperator` for `and` / `or`, breadth first.
    fn collect_logic_ops(&self, n: usize) -> Vec<usize> {
        let t = self.t;
        let mut ops = Vec::new();
        let mut queue = std::collections::VecDeque::from([n]);
        while let Some(n) = queue.pop_front() {
            if t.node_kind(n) == Some(Nk::BinaryExpression) {
                if let Some(op) = t.children(n).find(|&c| matches!(t.token_kind(c), Some(Tk::And | Tk::Or))) {
                    ops.push(op);
                }
                queue.extend(t.child_nodes(n, Nk::BinaryExpression));
            }
        }
        ops
    }

    fn align_chain_expr(&mut self, n: usize) {
        let t = self.t;
        if t.node_kind(t.parent(n)) != Some(Nk::ExpressionStatement)
            && self.style.align_chain_expr == AlignChainExpr::OnlyCallStmt
        {
            return;
        }
        let group = t.child_nodes(n, Nk::IndexExpression).into_iter().map(|i| t.first_token(i)).collect();
        self.push_group(Strategy::ToFirst, group);
    }

    fn align_continuous_similar_call_args(&mut self, block: usize) {
        let t = self.t;
        let space = self.style.align_continuous_line_space;
        let mut last_line = 0;
        let mut prefix = 0;
        let mut group: Vec<usize> = Vec::new();
        for stmt in t.child_nodes(block, Nk::ExpressionStatement) {
            let suffixed = t.child_node(stmt, Nk::SuffixedExpression);
            let call = t.last_child_node(suffixed, Nk::CallExpression);
            let simple = t.is_single_line(stmt);
            if group.is_empty() && simple {
                group.push(call);
                prefix = t.start_col(call).wrapping_sub(t.start_col(stmt));
                last_line = t.end_line(call);
                continue;
            }
            if simple {
                let line = t.start_line(call);
                let stmt_prefix = t.start_col(call).wrapping_sub(t.start_col(stmt));
                if line.wrapping_sub(last_line) <= space && stmt_prefix == prefix {
                    group.push(call);
                } else {
                    if group.len() > 1 {
                        self.align_similar_calls(std::mem::take(&mut group), prefix);
                    }
                    group.clear();
                    group.push(call);
                }
                prefix = stmt_prefix;
                last_line = t.end_line(call);
            } else {
                if group.len() > 1 {
                    self.align_similar_calls(std::mem::take(&mut group), prefix);
                }
                group.clear();
            }
        }
        if group.len() > 1 {
            self.align_similar_calls(group, prefix);
        }
    }

    fn align_similar_calls(&mut self, calls: Vec<usize>, prefix: usize) {
        let t = self.t;
        let s = self.style;
        let args_of: Vec<Vec<usize>> =
            calls.iter().map(|&c| t.child_exprs(t.child_node(c, Nk::ExpressionList))).collect();
        let max_align = args_of.iter().map(Vec::len).max().unwrap_or(0);
        let inside = s.space_inside_function_call_parentheses as usize;
        let mut align_pos = prefix + 1 + inside;
        for i in 0..max_align {
            let mut element = 0;
            let mut group = Vec::new();
            for args in &args_of {
                if let Some(&arg) = args.get(i) {
                    element = element.max(t.utf8_len(arg));
                    group.push(t.first_token(arg));
                }
            }
            self.push_normal_group(align_pos, group);
            align_pos += element;
            if i + 1 == max_align {
                align_pos += inside;
            } else {
                align_pos += if s.space_after_comma { 2 } else { 1 };
            }
        }
    }

    fn align_inline_comment(&mut self, n: usize) {
        let t = self.t;
        let prev = t.prev_token(n);
        if prev == 0 {
            return;
        }
        let line = t.start_line(n);
        if t.end_line(prev) != line || t.node_text(n).starts_with("---@") {
            return;
        }
        let groups = &mut self.align.inline_comments;
        let Some(top) = groups.last_mut().filter(|g| !g.is_empty()) else {
            if groups.is_empty() {
                groups.push(Vec::new());
            }
            groups.last_mut().unwrap().push(n);
            return;
        };
        let last = *top.last().unwrap();
        if line.wrapping_sub(t.end_line(last)) > self.style.align_continuous_line_space {
            groups.push(vec![n]);
        } else {
            top.push(n);
        }
    }
}
