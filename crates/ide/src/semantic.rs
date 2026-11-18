//! Semantic tokens, ported from LuaLS `script/core/semantic-tokens.lua` with its
//! legend (same type and modifier names and order), so themes colour both servers
//! alike.
//!
//! Only what the syntax tree decides is coloured: keywords (`true` / `false` / `nil`
//! read-only, `and` / `or` / `not` operators), operators, numbers (integers
//! `static`), strings with their escapes, comments and the `@tag` of a LuaCATS line,
//! parameter and local declarations (`<const>` read-only, `<close>` abstract, a
//! function value as a function), fields, methods, `goto` and labels.
//!
//! Left for name resolution (phase 4): uses of locals and parameters, globals and
//! library globals. A CfxLua hash string (`` `name` ``) is coloured as a string.

use crate::{Parse, TextRange, TextSize};
use cfx_lua_syntax::SyntaxKind::{self, *};
use cfx_lua_syntax::{SyntaxNode, SyntaxToken};

/// LuaLS `define.TokenTypes`, in legend order.
pub const TOKEN_TYPES: [&str; 23] = [
    "namespace",
    "type",
    "class",
    "enum",
    "interface",
    "struct",
    "typeParameter",
    "parameter",
    "variable",
    "property",
    "enumMember",
    "event",
    "function",
    "method",
    "macro",
    "keyword",
    "modifier",
    "comment",
    "string",
    "number",
    "regexp",
    "operator",
    "decorator",
];

/// LuaLS `define.TokenModifiers`, bit `i` is entry `i`.
pub const TOKEN_MODIFIERS: [&str; 11] = [
    "declaration",
    "definition",
    "readonly",
    "static",
    "deprecated",
    "abstract",
    "async",
    "modification",
    "documentation",
    "defaultLibrary",
    "global",
];

/// Index into `TOKEN_TYPES`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum TokenType {
    Struct = 5,
    TypeParameter = 6,
    Parameter = 7,
    Variable = 8,
    Property = 9,
    Function = 12,
    Method = 13,
    Keyword = 15,
    Comment = 17,
    String = 18,
    Number = 19,
    Operator = 21,
}

pub mod modifier {
    pub const DECLARATION: u32 = 1 << 0;
    pub const READONLY: u32 = 1 << 2;
    pub const STATIC: u32 = 1 << 3;
    /// An invalid escape in a string.
    pub const DEPRECATED: u32 = 1 << 4;
    pub const ABSTRACT: u32 = 1 << 5;
    /// A valid escape in a string.
    pub const MODIFICATION: u32 = 1 << 7;
    pub const DOCUMENTATION: u32 = 1 << 8;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HighlightToken {
    pub range: TextRange,
    pub ty: TokenType,
    pub modifiers: u32,
}

/// In source order, none overlapping, none empty. A token can span lines (long
/// strings and comments); the server splits it when the client needs that.
pub fn semantic_tokens(parse: &Parse) -> Vec<HighlightToken> {
    let mut out = Vec::new();
    for token in parse.syntax().descendants_with_tokens().filter_map(|e| e.into_token()) {
        classify(&token, &mut out);
    }
    out
}

fn classify(t: &SyntaxToken, out: &mut Vec<HighlightToken>) {
    let mut push = |range: TextRange, ty, modifiers| {
        if !range.is_empty() {
            out.push(HighlightToken { range, ty, modifiers });
        }
    };
    let range = t.text_range();
    let kind = t.kind();
    match kind {
        TRUE_KW | FALSE_KW | NIL_KW => push(range, TokenType::Keyword, modifier::READONLY),
        AND_KW | OR_KW | NOT_KW => push(range, TokenType::Operator, 0),
        LOCAL_KW => push(range, TokenType::Keyword, modifier::DECLARATION),
        _ if kind.is_keyword() => push(range, TokenType::Keyword, 0),
        NUMBER => {
            let integer = is_integer(t.text());
            push(range, TokenType::Number, if integer { modifier::STATIC } else { 0 });
        }
        STRING => {
            // The string in pieces around its escapes.
            let mut at = range.start();
            for (esc, valid) in escapes(t.text()) {
                let esc = esc + range.start();
                push(TextRange::new(at, esc.start()), TokenType::String, 0);
                let m = if valid { modifier::MODIFICATION } else { modifier::DEPRECATED };
                push(esc, TokenType::String, m);
                at = esc.end();
            }
            push(TextRange::new(at, range.end()), TokenType::String, 0);
        }
        LONG_STRING | HASH_STRING => push(range, TokenType::String, 0),
        COMMENT => match doc_tag(t.text()) {
            Some(tag) => {
                let tag = tag + range.start();
                push(TextRange::new(range.start(), tag.start()), TokenType::Comment, 0);
                push(tag, TokenType::Keyword, modifier::DOCUMENTATION);
            }
            None => push(range, TokenType::Comment, 0),
        },
        BLOCK_COMMENT | SHEBANG => push(range, TokenType::Comment, 0),
        NAME => {
            if let Some((ty, m)) = t.parent().and_then(|p| name_role(t, &p)) {
                push(range, ty, m);
            }
        }
        _ if kind.is_compound_assign() => push(range, TokenType::Operator, 0),
        // The operator of a binary or unary expression.
        _ if !kind.is_trivia() && t.parent().is_some_and(|p| matches!(p.kind(), BINARY_EXPR | UNARY_EXPR)) => {
            push(range, TokenType::Operator, 0);
        }
        _ => {}
    }
}

/// What a name is, from where it sits; `None` when that takes name resolution.
fn name_role(t: &SyntaxToken, parent: &SyntaxNode) -> Option<(TokenType, u32)> {
    use modifier::*;
    let after = |kind: SyntaxKind| prev_real_sibling(t).is_some_and(|p| p.kind() == kind);
    Some(match parent.kind() {
        PARAM => (TokenType::Parameter, DECLARATION),
        ATTRIB => (TokenType::TypeParameter, 0),
        LOCAL_FUNCTION_STAT => (TokenType::Function, DECLARATION),
        NUMERIC_FOR_STAT | GENERIC_FOR_STAT => (TokenType::Variable, DECLARATION),
        LOCAL_NAME => {
            let attrib = parent.children().find(|c| c.kind() == ATTRIB).and_then(|a| {
                a.children_with_tokens().filter_map(|e| e.into_token()).find(|t| t.kind() == NAME)
            });
            // A function value wins over the attribute, as in LuaLS.
            match attrib.as_ref().map(|a| a.text()) {
                _ if local_value_is_function(parent) => (TokenType::Function, DECLARATION),
                Some("const") => (TokenType::Variable, READONLY),
                Some("close") => (TokenType::Variable, ABSTRACT),
                _ => (TokenType::Variable, DECLARATION),
            }
        }
        FIELD_NAMED | FIELD_SET => (TokenType::Property, 0),
        METHOD_CALL_EXPR => (TokenType::Method, 0),
        INDEX_EXPR if after(DOT) => {
            if is_callee(parent) || is_assigned_function(parent) {
                (TokenType::Method, 0)
            } else {
                (TokenType::Property, 0)
            }
        }
        FUNC_NAME => {
            if after(COLON) {
                (TokenType::Method, DECLARATION)
            } else if after(DOT) {
                let last = next_real_sibling(t).is_none();
                if last { (TokenType::Method, 0) } else { (TokenType::Property, 0) }
            } else if next_real_sibling(t).is_none() {
                // `function name()`: a function, local or global.
                (TokenType::Function, 0)
            } else {
                return None;
            }
        }
        GOTO_STAT => (TokenType::Struct, 0),
        LABEL_STAT => (TokenType::Struct, DECLARATION),
        _ => return None,
    })
}

fn prev_real_sibling(t: &SyntaxToken) -> Option<SyntaxToken> {
    let mut e = t.prev_sibling_or_token();
    while let Some(x) = e {
        match x.as_token() {
            Some(tok) if tok.kind().is_trivia() => e = x.prev_sibling_or_token(),
            Some(tok) => return Some(tok.clone()),
            None => return None,
        }
    }
    None
}

fn next_real_sibling(t: &SyntaxToken) -> Option<SyntaxToken> {
    let mut e = t.next_sibling_or_token();
    while let Some(x) = e {
        match x.as_token() {
            Some(tok) if tok.kind().is_trivia() => e = x.next_sibling_or_token(),
            Some(tok) => return Some(tok.clone()),
            None => return None,
        }
    }
    None
}

/// `a.b(...)`: the field is what gets called.
fn is_callee(index: &SyntaxNode) -> bool {
    index.parent().is_some_and(|p| p.kind() == CALL_EXPR && p.first_child().as_ref() == Some(index))
}

/// `a.b = function() end`, matched by position in the two lists.
fn is_assigned_function(index: &SyntaxNode) -> bool {
    let Some(vars) = index.parent().filter(|p| p.kind() == VAR_LIST) else { return false };
    let Some(i) = vars.children().position(|c| &c == index) else { return false };
    let value = vars.parent().and_then(|stat| stat.children().find(|c| c.kind() == EXPR_LIST)).and_then(|l| l.children().nth(i));
    value.is_some_and(|v| v.kind() == FUNCTION_EXPR)
}

/// `local f = function() end`, matched by position.
fn local_value_is_function(local: &SyntaxNode) -> bool {
    let Some(stat) = local.parent() else { return false };
    let Some(i) = stat.children().filter(|c| c.kind() == LOCAL_NAME).position(|c| &c == local) else { return false };
    let value = stat.children().find(|c| c.kind() == EXPR_LIST).and_then(|l| l.children().nth(i));
    value.is_some_and(|v| v.kind() == FUNCTION_EXPR)
}

/// Lua reads a numeral as an integer unless it has a fraction or an exponent
/// (`p` in hex) or does not fit (then it is a float; not told apart here).
fn is_integer(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    match lower.strip_prefix("0x") {
        Some(hex) => !hex.contains(['.', 'p']),
        None => !lower.contains(['.', 'e']),
    }
}

/// Escapes in a quoted string, relative to its start, and whether Lua accepts each.
fn escapes(text: &str) -> Vec<(TextRange, bool)> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 1; // past the opening quote
    while i < b.len() {
        if b[i] != b'\\' {
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        let Some(&e) = b.get(i) else { break };
        let valid = match e {
            b'n' | b't' | b'r' | b'a' | b'b' | b'f' | b'v' | b'\\' | b'"' | b'\'' | b'z' => {
                i += 1;
                true
            }
            b'\n' | b'\r' => {
                i += 1;
                if b.get(i).is_some_and(|&n| (n == b'\n' || n == b'\r') && n != e) {
                    i += 1;
                }
                true
            }
            b'x' => {
                let ok = b.get(i + 1..i + 3).is_some_and(|h| h.iter().all(u8::is_ascii_hexdigit));
                i += if ok { 3 } else { 1 };
                ok
            }
            b'0'..=b'9' => {
                let digits = b[i..].iter().take(3).take_while(|d| d.is_ascii_digit()).count();
                let value: u32 = text[i..i + digits].parse().unwrap_or(256);
                i += digits;
                value <= 255
            }
            b'u' => match (b.get(i + 1), b[i..].iter().position(|&c| c == b'}')) {
                (Some(b'{'), Some(close)) => {
                    let ok = u32::from_str_radix(&text[i + 2..i + close], 16).is_ok_and(|v| v < (1 << 31));
                    i += close + 1;
                    ok
                }
                _ => {
                    i += 1;
                    false
                }
            },
            _ => {
                // A character, not a byte: never split a UTF-8 sequence.
                i += text[i..].chars().next().map_or(1, char::len_utf8);
                false
            }
        };
        out.push((TextRange::new(TextSize::from(start as u32), TextSize::from(i as u32)), valid));
    }
    out
}

/// The `@tag` (or `|`) of a LuaCATS line `---@tag ...` / `--- @tag` / `---|`, relative
/// to the comment's start; the tag colours as a keyword, the `---` as a comment.
fn doc_tag(text: &str) -> Option<TextRange> {
    let rest = text.strip_prefix("---")?;
    let skipped = rest.len() - rest.trim_start().len();
    let head = 3 + skipped;
    let after = text[head..].chars().next()?;
    if after != '@' && after != '|' {
        return None;
    }
    let name_len = text[head + 1..].find(char::is_whitespace).unwrap_or(text.len() - head - 1);
    Some(TextRange::new(TextSize::from(head as u32), TextSize::from((head + 1 + name_len) as u32)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;
    use modifier::*;

    /// (text, type, modifiers) of each token.
    fn tokens(text: &str) -> Vec<(String, TokenType, u32)> {
        semantic_tokens(&parse(text)).iter().map(|t| (text[t.range].to_string(), t.ty, t.modifiers)).collect()
    }

    fn of(text: &str, piece: &str) -> Vec<(TokenType, u32)> {
        tokens(text).into_iter().filter(|(s, _, _)| s == piece).map(|(_, t, m)| (t, m)).collect()
    }

    #[test]
    fn keywords_literals_and_operators() {
        let t = "local x = not true and 1 + 2.5 or nil";
        assert_eq!(of(t, "local"), [(TokenType::Keyword, DECLARATION)]);
        assert_eq!(of(t, "not"), [(TokenType::Operator, 0)]);
        assert_eq!(of(t, "true"), [(TokenType::Keyword, READONLY)]);
        assert_eq!(of(t, "1"), [(TokenType::Number, STATIC)]);
        assert_eq!(of(t, "2.5"), [(TokenType::Number, 0)]);
        assert_eq!(of(t, "+"), [(TokenType::Operator, 0)]);
        // `=` of a statement is not an operator.
        assert_eq!(of(t, "="), []);
        assert_eq!(of("x += 1", "+="), [(TokenType::Operator, 0)]);
    }

    #[test]
    fn declarations() {
        let t = "local a <const>, b <close>, c, f = 1, x, y, function() end\nlocal function g(p, ...) end\nfor i, v in t do end";
        assert_eq!(of(t, "a"), [(TokenType::Variable, READONLY)]);
        assert_eq!(of(t, "const"), [(TokenType::TypeParameter, 0)]);
        assert_eq!(of(t, "b"), [(TokenType::Variable, ABSTRACT)]);
        assert_eq!(of(t, "c"), [(TokenType::Variable, DECLARATION)]);
        assert_eq!(of(t, "f"), [(TokenType::Function, DECLARATION)]);
        assert_eq!(of(t, "g"), [(TokenType::Function, DECLARATION)]);
        assert_eq!(of(t, "p"), [(TokenType::Parameter, DECLARATION)]);
        assert_eq!(of(t, "i"), [(TokenType::Variable, DECLARATION)]);
        assert_eq!(of("local k <const> = function() end", "k"), [(TokenType::Function, DECLARATION)]);
        // Uses need name resolution: `t` and `x` are not coloured yet.
        assert_eq!(of(t, "x"), []);
    }

    #[test]
    fn fields_and_methods() {
        let t = "obj.prop = obj.run(1)\nobj.cb = function() end\nobj:go()\nfunction M.sub.fn() end\nfunction M:meth() end\nfunction top() end\nlocal t = { key = 1 }";
        assert_eq!(of(t, "prop"), [(TokenType::Property, 0)]);
        assert_eq!(of(t, "run"), [(TokenType::Method, 0)]);
        assert_eq!(of(t, "cb"), [(TokenType::Method, 0)]);
        assert_eq!(of(t, "go"), [(TokenType::Method, 0)]);
        assert_eq!(of(t, "sub"), [(TokenType::Property, 0)]);
        assert_eq!(of(t, "fn"), [(TokenType::Method, 0)]);
        assert_eq!(of(t, "meth"), [(TokenType::Method, DECLARATION)]);
        assert_eq!(of(t, "top"), [(TokenType::Function, 0)]);
        assert_eq!(of(t, "key"), [(TokenType::Property, 0)]);
        assert_eq!(of(t, "M"), []);
    }

    #[test]
    fn strings_comments_and_labels() {
        let t = "s = 'a\\n\\q\\65\\u{48}b' .. `h` .. [[x]]\n--- @param p any\n-- plain\n/* c */\ngoto done\n::done::";
        let all = tokens(t);
        let strs: Vec<_> = all.iter().filter(|(_, ty, _)| *ty == TokenType::String).map(|(s, _, m)| (s.as_str(), *m)).collect();
        assert_eq!(
            strs,
            [("'a", 0), ("\\n", MODIFICATION), ("\\q", DEPRECATED), ("\\65", MODIFICATION), ("\\u{48}", MODIFICATION), ("b'", 0), ("`h`", 0), ("[[x]]", 0)]
        );
        assert_eq!(of(t, "--- "), [(TokenType::Comment, 0)]);
        assert_eq!(of(t, "@param"), [(TokenType::Keyword, DOCUMENTATION)]);
        assert_eq!(of(t, "-- plain"), [(TokenType::Comment, 0)]);
        assert_eq!(of(t, "/* c */"), [(TokenType::Comment, 0)]);
        assert_eq!(of(t, "done"), [(TokenType::Struct, 0), (TokenType::Struct, DECLARATION)]);
    }

    #[test]
    fn tokens_are_ordered_and_disjoint() {
        let t = "local function f(a) return a .. 'x\\ty' end -- c\n";
        let toks = semantic_tokens(&parse(t));
        assert!(toks.windows(2).all(|w| w[0].range.end() <= w[1].range.start()));
    }
}
