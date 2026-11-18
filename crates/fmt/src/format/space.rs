//! `SpaceAnalyzer`: the spaces after each token. A setting on a token's right side
//! wins over an earlier one unless that one has a higher priority.

use super::{Fmt, NextSpace, Resolve};
use crate::style::{FunctionSingleArgSpace, SpaceAroundStyle, SpaceBeforeInlineCommentStyle};
use crate::tree::{Nk, Tk};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Priority {
    Normal,
    First,
}

pub(crate) struct State {
    right: Vec<Option<(usize, Priority)>>,
    ignore: Vec<bool>,
}

impl State {
    pub(super) fn new(n: usize) -> State {
        State { right: vec![None; n], ignore: vec![false; n] }
    }
}

impl Fmt<'_, '_> {
    pub(super) fn space_analyze(&mut self) {
        let t = self.t;
        let s = self.style;
        for n in t.all() {
            let Some(kind) = t.token_kind(n) else { continue };
            match kind {
                Tk::Plus | Tk::Star | Tk::Slash | Tk::Percent | Tk::Amp | Tk::Shl | Tk::Shr | Tk::IDiv => {
                    self.space_around(n, s.space_around_math_operator.other as usize);
                }
                Tk::Caret => self.space_around(n, s.space_around_math_operator.exponent as usize),
                Tk::Concat => {
                    let after_number = t.token_kind(t.prev_token(n)) == Some(Tk::Number);
                    match s.space_around_concat_operator {
                        SpaceAroundStyle::Always => self.space_around(n, 1),
                        SpaceAroundStyle::None => self.space_around(n, after_number as usize),
                        SpaceAroundStyle::NoSpaceAsym => {
                            if after_number {
                                self.space_left(n, 1);
                                self.space_right(n, 0);
                            } else {
                                self.space_around(n, 0);
                            }
                        }
                    }
                }
                Tk::Assign => {
                    let after_gt = t.token_kind(t.prev_token(n)) == Some(Tk::Gt);
                    match s.space_around_assign_operator {
                        SpaceAroundStyle::Always => self.space_around(n, 1),
                        SpaceAroundStyle::None => self.space_around(n, after_gt as usize),
                        SpaceAroundStyle::NoSpaceAsym => {
                            if after_gt {
                                self.space_left(n, 1);
                                self.space_right(n, 0);
                            } else {
                                self.space_around(n, 0);
                            }
                        }
                    }
                }
                Tk::Ge | Tk::Le | Tk::Ne | Tk::Eq => self.space_around(n, s.space_around_logical_operator as usize),
                Tk::And | Tk::Or | Tk::In => self.space_around(n, 1),
                Tk::Not => self.space_right(n, 1),
                Tk::Minus | Tk::Tilde => {
                    if t.node_kind(t.parent(n)) == Some(Nk::BinaryExpression) {
                        self.space_around(n, s.space_around_math_operator.other as usize);
                    } else {
                        self.space_right(n, 0);
                        let unary_next = t.node_kind(t.next_sibling(n)) == Some(Nk::UnaryExpression);
                        self.space_right(n, unary_next as usize);
                    }
                }
                Tk::Lt | Tk::Gt => {
                    if t.node_kind(t.parent(n)) == Some(Nk::BinaryExpression) {
                        self.space_around(n, s.space_around_logical_operator as usize);
                    } else if kind == Tk::Lt {
                        self.space_right(n, 0);
                    } else {
                        self.space_left(n, 0);
                    }
                }
                Tk::Comma => {
                    self.space_left(n, 0);
                    self.space_right(n, s.space_after_comma as usize);
                }
                Tk::Semi => {
                    self.space_left(n, 0);
                    self.space_right(n, 1);
                }
                Tk::If | Tk::Local | Tk::Elseif | Tk::Return | Tk::Goto | Tk::For | Tk::Else | Tk::Function | Tk::End => {
                    self.space_right(n, 1);
                }
                Tk::Then | Tk::Do | Tk::Until | Tk::While => self.space_around(n, 1),
                // CfxLua set constructor `{ .a, .b = 1 }`: the dot starts a field, so
                // the space before it is the separator's.
                Tk::Dot if t.node_kind(t.parent(n)) == Some(Nk::TableField) && t.first_child(t.parent(n)) == n => {
                    self.space_right(n, 0);
                }
                Tk::Dot | Tk::Colon | Tk::RBrack | Tk::LBrack => {
                    // C++ falls through from `.` `:` `]` into the `[` case.
                    if kind != Tk::LBrack {
                        self.space_around(n, 0);
                    }
                    let prev = t.token_kind(t.prev_token(n));
                    if matches!(prev, Some(Tk::Comma | Tk::Semi)) {
                        self.space_right(n, 0);
                    } else {
                        self.space_around(n, 0);
                    }
                }
                Tk::LParen => self.space_right(n, 0),
                Tk::RParen => self.space_left(n, 0),
                Tk::LongComment | Tk::ShortComment => {
                    let c = s.space_before_inline_comment;
                    if c.style == SpaceBeforeInlineCommentStyle::Fixed {
                        self.space_left_p(n, c.space, Priority::First);
                    } else {
                        let prev = t.prev_token(n);
                        if t.end_line(prev) == t.start_line(n) {
                            let space = t.start_col(n).wrapping_sub(t.end_col(prev)).wrapping_sub(1);
                            self.space_left_p(n, space, Priority::First);
                        } else {
                            self.space_left_p(n, 0, Priority::First);
                        }
                    }
                    self.space_right(n, 1);
                }
                Tk::Dots if t.token_kind(t.next_token(n)) == Some(Tk::Name) => self.space_right(n, 0),
                _ => {}
            }
        }
    }

    pub(super) fn space_complex_analyze(&mut self) {
        let t = self.t;
        let s = self.style;
        for n in t.all() {
            let Some(kind) = t.node_kind(n) else { continue };
            match kind {
                Nk::CallExpression => {
                    let lparen = t.child_token(n, Tk::LParen);
                    if t.is_token(lparen) {
                        if s.space_inside_function_call_parentheses
                            && t.token_kind(t.next_token(lparen)) != Some(Tk::RParen)
                        {
                            let rparen = t.child_token(n, Tk::RParen);
                            self.space_right(lparen, 1);
                            self.space_left(rparen, 1);
                        }
                        if s.ignore_spaces_inside_function_call {
                            let list = t.child_node(n, Nk::ExpressionList);
                            if t.is_node(list) {
                                for comma in t.child_tokens(list, Tk::Comma) {
                                    self.space.ignore[comma] = true;
                                }
                            }
                        }
                        let left = t.token_kind(t.prev_token(lparen));
                        if left != Some(Tk::String) && left != Some(Tk::RCurly) {
                            if s.space_before_function_call_open_parenthesis {
                                self.space_left(lparen, 1);
                            } else if !self.token_is_remove(lparen) {
                                self.space_left(lparen, 0);
                            }
                        } else {
                            self.space_left(lparen, 1);
                        }
                    } else {
                        self.function_call_single_arg_space(n);
                    }
                }
                Nk::TableExpression => {
                    let lcurly = t.child_token(n, Tk::LCurly);
                    let rcurly = t.child_token(n, Tk::RCurly);
                    if t.token_kind(t.next_token(lcurly)) != Some(Tk::RCurly) && s.space_around_table_field_list {
                        self.space_right(lcurly, 1);
                        self.space_left(rcurly, 1);
                    } else {
                        self.space_right(lcurly, 0);
                        self.space_left(rcurly, 0);
                    }
                }
                Nk::IndexExpression => {
                    let lbrack = t.child_token(n, Tk::LBrack);
                    let rbrack = t.child_token(n, Tk::RBrack);
                    if t.is_token(lbrack) && t.is_token(rbrack) {
                        if s.space_before_open_square_bracket {
                            self.space_left(lbrack, 1);
                        }
                        if s.space_inside_square_brackets {
                            self.space_right(lbrack, 1);
                            self.space_left(rbrack, 1);
                        } else {
                            let after = t.token_kind(t.next_token(lbrack));
                            let before = t.token_kind(t.prev_token(rbrack));
                            let long = |k: Option<Tk>| matches!(k, Some(Tk::LongString | Tk::LongComment));
                            if long(after) || long(before) {
                                self.space_right(lbrack, 1);
                                self.space_left(rbrack, 1);
                            }
                        }
                        if s.space_around_table_append_operator {
                            let binary = t.child_node(n, Nk::BinaryExpression);
                            if t.is_node(binary) {
                                let plus = t.child_token(binary, Tk::Plus);
                                if t.is_token(plus) {
                                    let exprs = t.child_exprs(binary);
                                    if let [left, right] = exprs[..]
                                        && t.node_kind(left) == Some(Nk::UnaryExpression)
                                        && t.is_token(t.child_token(left, Tk::Hash))
                                        && t.node_kind(right) == Some(Nk::LiteralExpression)
                                        && t.node_text(right) == "1"
                                    {
                                        self.space_around(plus, 0);
                                    }
                                }
                            }
                        }
                    }
                    if s.ignore_space_after_colon {
                        let colon = t.child_token(n, Tk::Colon);
                        if t.is_token(colon) {
                            self.space.ignore[colon] = true;
                        }
                    }
                }
                Nk::Attribute => self.space_left(n, s.space_before_attribute as usize),
                Nk::FunctionBody => {
                    let lparen = t.child_token(n, Tk::LParen);
                    let closure = t.node_kind(t.parent(n)) == Some(Nk::ClosureExpression);
                    let before = if closure {
                        s.space_before_closure_open_parenthesis
                    } else {
                        s.space_before_function_open_parenthesis
                    };
                    self.space_left(lparen, before as usize);
                    if s.space_inside_function_param_list_parentheses
                        && t.token_kind(t.next_token(lparen)) != Some(Tk::RParen)
                    {
                        self.space_right(lparen, 1);
                        let rparen = t.child_token(n, Tk::RParen);
                        self.space_left(rparen, 1);
                    }
                }
                Nk::ForNumber if !s.space_after_comma_in_for_statement => {
                    for comma in t.child_tokens(n, Tk::Comma) {
                        self.space_right(comma, 0);
                    }
                }
                Nk::ForList if !s.space_after_comma_in_for_statement => {
                    let names = t.child_node(n, Nk::NameDefList);
                    for comma in t.child_tokens(names, Tk::Comma) {
                        self.space_right(comma, 0);
                    }
                    let exprs = t.child_node(n, Nk::ExpressionList);
                    for comma in t.child_tokens(exprs, Tk::Comma) {
                        self.space_right(comma, 0);
                    }
                }
                _ => {}
            }
        }
    }

    pub(super) fn space_query(&mut self, n: usize, r: &mut Resolve) {
        let t = self.t;
        if t.is_token(n) {
            let next = t.next_token(n);
            r.next_space = NextSpace::Space(self.process_space(n, next));
        }
    }

    pub(super) fn space_around(&mut self, n: usize, space: usize) {
        self.space_left(n, space);
        self.space_right(n, space);
    }

    pub(super) fn space_around_p(&mut self, n: usize, space: usize, p: Priority) {
        self.space_left_p(n, space, p);
        self.space_right_p(n, space, p);
    }

    pub(super) fn space_left(&mut self, n: usize, space: usize) {
        self.space_left_p(n, space, Priority::Normal);
    }

    pub(super) fn space_left_p(&mut self, n: usize, space: usize, p: Priority) {
        let token = self.t.prev_token(n);
        self.set_right(token, space, p);
    }

    pub(super) fn space_right(&mut self, n: usize, space: usize) {
        self.space_right_p(n, space, Priority::Normal);
    }

    pub(super) fn space_right_p(&mut self, n: usize, space: usize, p: Priority) {
        let token = self.t.last_token(n);
        self.set_right(token, space, p);
    }

    fn set_right(&mut self, token: usize, space: usize, p: Priority) {
        if let Some((_, old)) = self.space.right[token]
            && old > p
        {
            return;
        }
        self.space.right[token] = Some((space, p));
    }

    pub(super) fn space_ignore(&mut self, n: usize) {
        self.space.ignore[n] = true;
    }

    pub(super) fn function_call_single_arg_space(&mut self, n: usize) {
        let t = self.t;
        let first = t.first_token(n);
        let setting = if matches!(t.token_kind(first), Some(Tk::String | Tk::LongString)) {
            self.style.space_before_function_call_single_arg.string
        } else {
            self.style.space_before_function_call_single_arg.table
        };
        match setting {
            FunctionSingleArgSpace::None => self.space_left(first, 0),
            FunctionSingleArgSpace::Always => self.space_left(first, 1),
            FunctionSingleArgSpace::Keep => self.space_ignore(first),
        }
    }

    fn process_space(&self, left: usize, right: usize) -> usize {
        if !self.space.ignore[left]
            && let Some((space, _)) = self.space.right[left]
        {
            return space;
        }
        if right != 0 {
            let t = self.t;
            return t.start_offset(right).wrapping_sub(t.end_offset(left)).wrapping_sub(1);
        }
        0
    }
}
