//! CfxLua grammar: Lua 5.4 (`lparser.c`) plus the power patches FiveM builds
//! with (`code/vendor/lua.lua` in citizenfx/fivem):
//! - compound assignment `a += 1` (single target only)
//! - `in` unpacking `local a, b in t` / `a, b in t`
//! - set constructors `{ .a, .b = 2, [k] }` (a missing `= value` means `true`)
//! - safe navigation `a?.b`, `a?[k]`
//! - back-tick hashes `` `adder` `` (a literal)

use crate::SyntaxKind::{self, *};
use crate::parser::{CompletedMarker, Parser};

pub(crate) fn source_file(p: &mut Parser) {
    let m = p.start();
    block(p, true);
    m.complete(p, SOURCE_FILE);
}

fn at_block_end(p: &Parser) -> bool {
    p.at_any(&[EOF, END_KW, ELSE_KW, ELSEIF_KW, UNTIL_KW])
}

fn block(p: &mut Parser, top_level: bool) {
    let m = p.start();
    // Token span of a `return` that is not the last statement yet.
    let mut returned: Option<(usize, usize)> = None;
    loop {
        match p.current() {
            EOF => break,
            END_KW | ELSE_KW | ELSEIF_KW | UNTIL_KW => {
                if top_level {
                    p.err_and_bump("UNEXPECT_SYMBOL", "unexpected keyword, expected end of file");
                } else {
                    break;
                }
            }
            _ => {
                if let Some((from, to)) = returned.take() {
                    p.error_span("ACTION_AFTER_RETURN", "'return' must be the last statement of a block", from, to);
                }
                if p.at(RETURN_KW) {
                    let from = p.token_pos();
                    return_stat(p);
                    returned = Some((from, p.token_pos()));
                } else {
                    statement(p);
                }
            }
        }
    }
    m.complete(p, BLOCK);
}

fn statement(p: &mut Parser) {
    if !p.enter() {
        p.err_and_bump("TOO_DEEP", "too deeply nested");
        return;
    }
    match p.current() {
        SEMICOLON => {
            let m = p.start();
            p.bump();
            m.complete(p, EMPTY_STAT);
        }
        IF_KW => if_stat(p),
        WHILE_KW => {
            let m = p.start();
            p.bump();
            expr(p);
            p.expect(DO_KW);
            block(p, false);
            p.expect(END_KW);
            m.complete(p, WHILE_STAT);
        }
        DO_KW => {
            let m = p.start();
            p.bump();
            block(p, false);
            p.expect(END_KW);
            m.complete(p, DO_STAT);
        }
        FOR_KW => for_stat(p),
        REPEAT_KW => {
            let m = p.start();
            p.bump();
            block(p, false);
            p.expect(UNTIL_KW);
            expr(p);
            m.complete(p, REPEAT_STAT);
        }
        FUNCTION_KW => function_stat(p),
        LOCAL_KW => local_stat(p),
        COLON2 => {
            let m = p.start();
            p.bump();
            p.expect_name();
            p.expect(COLON2);
            m.complete(p, LABEL_STAT);
        }
        BREAK_KW => {
            let m = p.start();
            p.bump();
            m.complete(p, BREAK_STAT);
        }
        GOTO_KW => {
            let m = p.start();
            p.bump();
            p.expect_name();
            m.complete(p, GOTO_STAT);
        }
        _ => expr_stat(p),
    }
    p.leave();
}

fn if_stat(p: &mut Parser) {
    let m = p.start();
    p.bump();
    expr(p);
    p.expect(THEN_KW);
    block(p, false);
    while p.at(ELSEIF_KW) {
        let c = p.start();
        p.bump();
        expr(p);
        p.expect(THEN_KW);
        block(p, false);
        c.complete(p, ELSEIF_CLAUSE);
    }
    if p.at(ELSE_KW) {
        let c = p.start();
        p.bump();
        block(p, false);
        c.complete(p, ELSE_CLAUSE);
    }
    p.expect(END_KW);
    m.complete(p, IF_STAT);
}

fn for_stat(p: &mut Parser) {
    let m = p.start();
    p.bump();
    p.expect_name();
    if p.eat(EQ) {
        expr(p);
        p.expect(COMMA);
        expr(p);
        if p.eat(COMMA) {
            expr(p);
        }
        p.expect(DO_KW);
        block(p, false);
        p.expect(END_KW);
        m.complete(p, NUMERIC_FOR_STAT);
    } else {
        while p.eat(COMMA) {
            p.expect_name();
        }
        p.expect(IN_KW);
        expr_list(p);
        p.expect(DO_KW);
        block(p, false);
        p.expect(END_KW);
        m.complete(p, GENERIC_FOR_STAT);
    }
}

fn function_stat(p: &mut Parser) {
    let m = p.start();
    p.bump();
    let n = p.start();
    p.expect_name();
    loop {
        if p.eat(DOT) {
            p.expect_name();
        } else if p.at(L_BRACK) {
            // `function t[k]() end` is not Lua; parse the index so the rest of the
            // function still reads.
            let from = p.token_pos();
            let e = p.start();
            p.bump();
            if p.at(R_BRACK) {
                p.error("MISS_EXP", "expected an expression");
            } else {
                expr(p);
            }
            p.expect(R_BRACK);
            e.complete(p, ERROR);
            let msg = "a function name cannot use '[...]'; write `t[k] = function() ... end`";
            p.error_span("INDEX_IN_FUNC_NAME", msg, from, p.token_pos());
        } else {
            break;
        }
    }
    if p.eat(COLON) {
        p.expect_name();
    }
    n.complete(p, FUNC_NAME);
    func_body(p);
    m.complete(p, FUNCTION_STAT);
}

fn local_stat(p: &mut Parser) {
    let m = p.start();
    p.bump();
    if p.eat(FUNCTION_KW) {
        p.expect_name();
        func_body(p);
        m.complete(p, LOCAL_FUNCTION_STAT);
        return;
    }
    // `>=` straight after an attribute (`local x <const>= 1`) closes it and is the `=`.
    let mut eq_taken = false;
    loop {
        let n = p.start();
        if p.at(LT) {
            // Lua 5.5 writes the attribute first (`local <const> x`); 5.4 does not.
            let from = p.token_pos();
            attrib(p);
            let msg = "Lua 5.4 writes the attribute after the name: `local x <const>`";
            p.error_span("UNSUPPORT_SYMBOL", msg, from, p.token_pos());
            p.expect_name();
        } else {
            p.expect_name();
            if p.at(LT) {
                eq_taken = attrib(p);
            }
        }
        n.complete(p, LOCAL_NAME);
        if eq_taken || !p.eat(COMMA) {
            break;
        }
    }
    if eq_taken || p.eat(EQ) {
        expr_list(p);
    } else if p.eat(IN_KW) {
        unpack_source(p);
    }
    m.complete(p, LOCAL_STAT);
}

/// `<name>`. Returns true when it ended with `>=` (that `=` belongs to the statement).
fn attrib(p: &mut Parser) -> bool {
    let a = p.start();
    p.bump();
    p.expect_name();
    let mut eq_taken = false;
    if p.at(GTEQ) {
        let at = p.token_pos();
        p.error_span("MISS_SPACE_BETWEEN", "write a space between '>' and '='", at, at + 1);
        p.bump();
        eq_taken = true;
    } else {
        p.expect(GT);
    }
    a.complete(p, ATTRIB);
    eq_taken
}

/// The table after `in` (a suffixed expression in lparser.c).
fn unpack_source(p: &mut Parser) {
    if p.at_any(&[NAME, L_PAREN]) {
        suffixed_expr(p);
    } else {
        expr_error(p);
    }
}

fn return_stat(p: &mut Parser) {
    let m = p.start();
    p.bump();
    // A keyword that only starts a statement is not a missing expression: the block
    // reports the statement after `return` instead.
    const STATEMENT_ONLY: &[SyntaxKind] =
        &[SEMICOLON, LOCAL_KW, GOTO_KW, IF_KW, FOR_KW, WHILE_KW, REPEAT_KW, DO_KW, BREAK_KW, RETURN_KW, COLON2];
    if !at_block_end(p) && !p.at_any(STATEMENT_ONLY) {
        expr_list(p);
    }
    p.eat(SEMICOLON);
    m.complete(p, RETURN_STAT);
}

fn expr_stat(p: &mut Parser) {
    if !p.at_any(&[NAME, L_PAREN]) {
        p.err_and_bump("UNEXPECT_SYMBOL", "unexpected symbol");
        return;
    }
    let m = p.start();
    let Some(first) = suffixed_expr(p) else {
        m.abandon(p);
        return;
    };
    let kind = p.current();
    if kind.is_compound_assign() {
        check_assignable(p, first);
        p.bump();
        expr(p);
        m.complete(p, COMPOUND_ASSIGN_STAT);
    } else if kind == DOT2 && p.nth(1) == EQ {
        // `s ..= x`: other Lua forks have it, FiveM does not build that patch.
        let at = p.token_pos();
        p.error_span("UNSUPPORT_SYMBOL", "`..=` is not supported in CfxLua; write `s = s .. x`", at, at + 2);
        p.bump();
        p.bump();
        expr(p);
        m.complete(p, COMPOUND_ASSIGN_STAT);
    } else if matches!(kind, EQ | COMMA | IN_KW) {
        check_assignable(p, first);
        let list = first.precede(p);
        while p.eat(COMMA) {
            if let Some(v) = target(p) {
                check_assignable(p, v);
            }
        }
        list.complete(p, VAR_LIST);
        if p.eat(IN_KW) {
            unpack_source(p);
        } else if p.current().is_compound_assign() {
            p.error("COMPOUND_MULTI_TARGET", "compound assignment is not allowed on more than one target");
            p.bump();
            expr(p);
        } else {
            p.expect(EQ);
            expr_list(p);
        }
        m.complete(p, ASSIGN_STAT);
    } else {
        if !matches!(first.kind(), CALL_EXPR | METHOD_CALL_EXPR) {
            p.error("EXP_IN_ACTION", "syntax error: this expression is not a statement");
        }
        m.complete(p, CALL_STAT);
    }
}

fn target(p: &mut Parser) -> Option<CompletedMarker> {
    if p.at_any(&[NAME, L_PAREN]) {
        suffixed_expr(p)
    } else {
        expr_error(p);
        None
    }
}

fn check_assignable(p: &mut Parser, m: CompletedMarker) {
    if !matches!(m.kind(), NAME_EXPR | INDEX_EXPR) {
        p.error("UNASSIGNABLE", "syntax error: cannot assign to this expression");
    }
}

fn expr_list(p: &mut Parser) {
    let m = p.start();
    expr(p);
    while p.eat(COMMA) {
        expr(p);
    }
    m.complete(p, EXPR_LIST);
}

fn func_body(p: &mut Parser) {
    let m = p.start();
    if p.at(L_PAREN) {
        let l = p.start();
        p.bump();
        if !p.at(R_PAREN) {
            loop {
                match p.current() {
                    NAME => {
                        let a = p.start();
                        p.bump();
                        a.complete(p, PARAM);
                    }
                    DOT3 => {
                        let a = p.start();
                        p.bump();
                        if p.at(NAME) {
                            let at = p.token_pos();
                            p.error_span("UNSUPPORT_NAMED_VARARG", "named varargs (`...name`) are Lua 5.5", at, at + 1);
                            p.bump();
                        }
                        a.complete(p, PARAM);
                        break;
                    }
                    TRUE_KW | FALSE_KW | NIL_KW | AND_KW | OR_KW | NOT_KW => {
                        let a = p.start();
                        p.expect_name();
                        a.complete(p, PARAM);
                    }
                    _ => {
                        p.error("MISS_NAME", "expected a parameter name");
                        break;
                    }
                }
                if !p.eat(COMMA) {
                    break;
                }
            }
        }
        p.expect(R_PAREN);
        l.complete(p, PARAM_LIST);
    } else {
        p.error("MISS_SYMBOL", "expected '('");
    }
    block(p, false);
    p.expect(END_KW);
    m.complete(p, FUNC_BODY);
}

pub(crate) fn expr(p: &mut Parser) -> Option<CompletedMarker> {
    sub_expr(p, 0)
}

fn unary_op(kind: SyntaxKind) -> bool {
    matches!(kind, NOT_KW | MINUS | HASH | TILDE)
}

/// (left, right) priorities from lparser.c.
fn binary_priority(kind: SyntaxKind) -> Option<(u8, u8)> {
    Some(match kind {
        PLUS | MINUS => (10, 10),
        STAR | PERCENT | SLASH | SLASH2 => (11, 11),
        CARET => (14, 13),
        AMP => (6, 6),
        PIPE => (4, 4),
        TILDE => (5, 5),
        SHL | SHR => (7, 7),
        DOT2 => (9, 8),
        EQ2 | LT | LTEQ | NEQ | GT | GTEQ => (3, 3),
        AND_KW => (2, 2),
        OR_KW => (1, 1),
        _ => return None,
    })
}

const UNARY_PRIORITY: u8 = 12;

fn sub_expr(p: &mut Parser, limit: u8) -> Option<CompletedMarker> {
    if !p.enter() {
        return None;
    }
    let lhs = if unary_op(p.current()) {
        let m = p.start();
        p.bump();
        sub_expr(p, UNARY_PRIORITY);
        Some(m.complete(p, UNARY_EXPR))
    } else {
        simple_expr(p)
    };
    let Some(mut lhs) = lhs else {
        p.leave();
        return None;
    };
    while let Some((left, right)) = binary_priority(p.current()) {
        if left <= limit {
            break;
        }
        let m = lhs.precede(p);
        p.bump();
        sub_expr(p, right);
        lhs = m.complete(p, BINARY_EXPR);
    }
    p.leave();
    Some(lhs)
}

fn simple_expr(p: &mut Parser) -> Option<CompletedMarker> {
    match p.current() {
        STRING | LONG_STRING => {
            let from = p.token_pos();
            let m = p.start();
            p.bump();
            let lit = m.complete(p, LITERAL_EXPR);
            Some(need_paren(p, lit, from))
        }
        NUMBER | HASH_STRING | NIL_KW | TRUE_KW | FALSE_KW => {
            let m = p.start();
            p.bump();
            Some(m.complete(p, LITERAL_EXPR))
        }
        DOT3 => {
            let m = p.start();
            p.bump();
            Some(m.complete(p, VARARG_EXPR))
        }
        L_CURLY => {
            let from = p.token_pos();
            let t = table_expr(p);
            Some(need_paren(p, t, from))
        }
        FUNCTION_KW => {
            let m = p.start();
            p.bump();
            func_body(p);
            Some(m.complete(p, FUNCTION_EXPR))
        }
        NAME | L_PAREN => suffixed_expr(p),
        _ => {
            expr_error(p);
            None
        }
    }
}

/// A string or table constructor followed by `.` or `:` (`"x":rep(2)`): Lua needs
/// parentheses there. Reported, then parsed as if they were present.
fn need_paren(p: &mut Parser, lhs: CompletedMarker, from: usize) -> CompletedMarker {
    if !p.at_any(&[DOT, COLON]) {
        return lhs;
    }
    p.error_span("NEED_PAREN", "wrap this in parentheses to index it or call a method on it", from, p.token_pos());
    suffixes(p, lhs)
}

/// Report a missing expression; swallow the token unless it can end/start something.
fn expr_error(p: &mut Parser) {
    const RECOVERY: &[SyntaxKind] = &[
        EOF, END_KW, ELSE_KW, ELSEIF_KW, UNTIL_KW, THEN_KW, DO_KW, LOCAL_KW, IF_KW, FOR_KW, WHILE_KW, REPEAT_KW,
        RETURN_KW, BREAK_KW, GOTO_KW, R_PAREN, R_CURLY, R_BRACK, COMMA, SEMICOLON, EQ, COLON2,
    ];
    if p.at_any(RECOVERY) {
        p.error("MISS_EXP", "expected an expression");
    } else {
        p.err_and_bump("MISS_EXP", "expected an expression");
    }
}

fn primary_expr(p: &mut Parser) -> Option<CompletedMarker> {
    match p.current() {
        NAME => {
            let m = p.start();
            p.bump();
            Some(m.complete(p, NAME_EXPR))
        }
        L_PAREN => {
            let m = p.start();
            p.bump();
            expr(p);
            p.expect(R_PAREN);
            Some(m.complete(p, PAREN_EXPR))
        }
        _ => {
            expr_error(p);
            None
        }
    }
}

fn suffixed_expr(p: &mut Parser) -> Option<CompletedMarker> {
    let lhs = primary_expr(p)?;
    Some(suffixes(p, lhs))
}

fn suffixes(p: &mut Parser, mut lhs: CompletedMarker) -> CompletedMarker {
    loop {
        lhs = match p.current() {
            DOT => {
                let m = lhs.precede(p);
                p.bump();
                p.expect(NAME);
                m.complete(p, INDEX_EXPR)
            }
            L_BRACK => {
                let m = lhs.precede(p);
                p.bump();
                expr(p);
                p.expect(R_BRACK);
                m.complete(p, INDEX_EXPR)
            }
            QUESTION => {
                let m = lhs.precede(p);
                p.bump();
                match p.current() {
                    DOT => {
                        p.bump();
                        p.expect(NAME);
                    }
                    L_BRACK => {
                        p.bump();
                        expr(p);
                        p.expect(R_BRACK);
                    }
                    _ => p.error("MISS_SYMBOL", "expected '.' or '[' after '?'"),
                }
                m.complete(p, INDEX_EXPR)
            }
            COLON => {
                let m = lhs.precede(p);
                p.bump();
                p.expect(NAME);
                call_args(p);
                m.complete(p, METHOD_CALL_EXPR)
            }
            L_PAREN | STRING | LONG_STRING | L_CURLY => {
                let m = lhs.precede(p);
                call_args(p);
                m.complete(p, CALL_EXPR)
            }
            _ => break,
        };
    }
    lhs
}

fn call_args(p: &mut Parser) {
    let m = p.start();
    match p.current() {
        L_PAREN => {
            p.bump();
            if !p.at(R_PAREN) {
                expr(p);
                while p.eat(COMMA) {
                    expr(p);
                }
            }
            p.expect(R_PAREN);
        }
        STRING | LONG_STRING => p.bump(),
        L_CURLY => {
            table_expr(p);
        }
        _ => p.error("MISS_SYMBOL", "expected function arguments"),
    }
    m.complete(p, ARG_LIST);
}

fn table_expr(p: &mut Parser) -> CompletedMarker {
    let m = p.start();
    p.bump(); // {
    while !p.at(R_CURLY) && !p.at(EOF) {
        let before = p.token_pos();
        field(p);
        if p.eat(COMMA) || p.eat(SEMICOLON) {
            continue;
        }
        if p.at(R_CURLY) || p.token_pos() == before {
            break;
        }
        // Missing separator: Lua expects '}' here. Report and keep parsing fields.
        p.error("MISS_SYMBOL", "expected '}' or ','");
        if !p.at_any(&[NAME, L_BRACK, DOT, L_CURLY, STRING, LONG_STRING, NUMBER, FUNCTION_KW, NOT_KW, MINUS]) {
            break;
        }
    }
    p.expect(R_CURLY);
    m.complete(p, TABLE_EXPR)
}

fn field(p: &mut Parser) {
    match p.current() {
        L_BRACK => {
            let m = p.start();
            p.bump();
            expr(p);
            p.expect(R_BRACK);
            // With GRIT_POWER_TABINIT, `{ [k] }` is valid and means `{ [k] = true }`.
            if p.eat(EQ) {
                expr(p);
            }
            m.complete(p, FIELD_INDEXED);
        }
        NAME if p.nth(1) == EQ => {
            let m = p.start();
            p.bump();
            p.bump();
            expr(p);
            m.complete(p, FIELD_NAMED);
        }
        DOT => {
            let m = p.start();
            p.bump();
            p.expect(NAME);
            if p.eat(EQ) {
                expr(p);
            }
            m.complete(p, FIELD_SET);
        }
        _ => {
            let m = p.start();
            if expr(p).is_some() {
                m.complete(p, FIELD_POS);
            } else {
                m.abandon(p);
            }
        }
    }
}
