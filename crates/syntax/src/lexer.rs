//! CfxLua lexer. Produces every byte of the input as a token (trivia included),
//! so the tree built from it reproduces the source exactly.
//!
//! Follows `llex.c` of citizenfx/lua (branch `luaglm-dev/cfx`) with the power
//! patches FiveM enables: compound operators, C-style block comments, safe
//! navigation `?` and back-tick hashes.

use crate::SyntaxKind::{self, *};
use rowan::{TextRange, TextSize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub kind: SyntaxKind,
    pub len: TextSize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexError {
    pub range: TextRange,
    /// Stable code (LuaLS error type names where one exists).
    pub code: &'static str,
    pub message: String,
}

pub fn tokenize(text: &str) -> (Vec<Token>, Vec<LexError>) {
    let mut lexer = Lexer { src: text.as_bytes(), pos: 0, errors: Vec::new() };
    let mut tokens = Vec::with_capacity(text.len() / 3);
    // A UTF-8 BOM is skipped, as luaL_loadfile and LuaLS do. It stays in the tree as trivia.
    if lexer.src.starts_with("\u{feff}".as_bytes()) {
        lexer.pos = 3;
        tokens.push(Token { kind: WHITESPACE, len: TextSize::from(3) });
    }
    if lexer.src[lexer.pos..].starts_with(b"#") {
        // Lua skips the whole first line when it starts with '#'.
        let start = lexer.pos;
        lexer.eat_line();
        tokens.push(Token { kind: SHEBANG, len: TextSize::from((lexer.pos - start) as u32) });
    }
    while lexer.pos < lexer.src.len() {
        let start = lexer.pos;
        let kind = lexer.next_kind();
        debug_assert!(lexer.pos > start, "lexer made no progress");
        tokens.push(Token { kind, len: TextSize::from((lexer.pos - start) as u32) });
    }
    (tokens, lexer.errors)
}

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    errors: Vec<LexError>,
}

impl Lexer<'_> {
    fn peek(&self, n: usize) -> u8 {
        self.src.get(self.pos + n).copied().unwrap_or(0)
    }

    fn at_end(&self) -> bool {
        self.pos >= self.src.len()
    }

    fn error(&mut self, start: usize, code: &'static str, message: impl Into<String>) {
        self.errors.push(LexError {
            range: TextRange::new((start as u32).into(), (self.pos as u32).into()),
            code,
            message: message.into(),
        });
    }

    fn eat_line(&mut self) {
        while !self.at_end() && !matches!(self.peek(0), b'\n' | b'\r') {
            self.pos += 1;
        }
    }

    /// Advance by `n` bytes and return `kind`.
    fn op(&mut self, n: usize, kind: SyntaxKind) -> SyntaxKind {
        self.pos += n;
        kind
    }

    fn next_kind(&mut self) -> SyntaxKind {
        let start = self.pos;
        let c = self.peek(0);
        match c {
            b' ' | b'\t' | b'\r' | b'\n' | 0x0b | 0x0c => {
                while matches!(self.peek(0), b' ' | b'\t' | b'\r' | b'\n' | 0x0b | 0x0c) && !self.at_end() {
                    self.pos += 1;
                }
                WHITESPACE
            }
            b'-' => match self.peek(1) {
                b'=' => self.op(2, MINUS_EQ),
                b'-' => {
                    self.pos += 2;
                    if self.peek(0) == b'['
                        && let Some(level) = self.long_bracket_level()
                    {
                        self.read_long_bracket(start, level, "comment");
                        return BLOCK_COMMENT;
                    }
                    self.eat_line();
                    COMMENT
                }
                _ => self.op(1, MINUS),
            },
            b'/' => match self.peek(1) {
                b'/' => self.op(2, SLASH2),
                b'=' => self.op(2, SLASH_EQ),
                b'*' => {
                    self.pos += 2;
                    loop {
                        if self.at_end() {
                            self.error(start, "MISS_SYMBOL", "unfinished long comment");
                            break;
                        }
                        if self.peek(0) == b'*' && self.peek(1) == b'/' {
                            self.pos += 2;
                            break;
                        }
                        self.pos += 1;
                    }
                    BLOCK_COMMENT
                }
                _ => self.op(1, SLASH),
            },
            b'[' => match self.long_bracket_level() {
                Some(level) => {
                    self.read_long_bracket(start, level, "string");
                    LONG_STRING
                }
                None => {
                    // `[=` that is not a long bracket is an error in Lua ("invalid long string delimiter").
                    if self.peek(1) == b'=' {
                        self.pos += 1;
                        while self.peek(0) == b'=' {
                            self.pos += 1;
                        }
                        self.error(start, "MISS_SYMBOL", "invalid long string delimiter");
                        return ERROR_TOKEN;
                    }
                    self.op(1, L_BRACK)
                }
            },
            b'=' => match self.peek(1) {
                b'=' => self.op(2, EQ2),
                _ => self.op(1, EQ),
            },
            b'<' => match (self.peek(1), self.peek(2)) {
                (b'=', _) => self.op(2, LTEQ),
                (b'<', b'=') => self.op(3, SHL_EQ),
                (b'<', _) => self.op(2, SHL),
                _ => self.op(1, LT),
            },
            b'>' => match (self.peek(1), self.peek(2)) {
                (b'=', _) => self.op(2, GTEQ),
                (b'>', b'=') => self.op(3, SHR_EQ),
                (b'>', _) => self.op(2, SHR),
                _ => self.op(1, GT),
            },
            b'+' => match self.peek(1) {
                b'=' => self.op(2, PLUS_EQ),
                _ => self.op(1, PLUS),
            },
            b'*' => match self.peek(1) {
                b'=' => self.op(2, STAR_EQ),
                _ => self.op(1, STAR),
            },
            b'&' => match self.peek(1) {
                b'=' => self.op(2, AMP_EQ),
                _ => self.op(1, AMP),
            },
            b'|' => match self.peek(1) {
                b'=' => self.op(2, PIPE_EQ),
                _ => self.op(1, PIPE),
            },
            b'^' => match self.peek(1) {
                b'=' => self.op(2, CARET_EQ),
                _ => self.op(1, CARET),
            },
            b'~' => match self.peek(1) {
                b'=' => self.op(2, NEQ),
                _ => self.op(1, TILDE),
            },
            b':' => match self.peek(1) {
                b':' => self.op(2, COLON2),
                _ => self.op(1, COLON),
            },
            b'%' => self.op(1, PERCENT),
            b'#' => self.op(1, HASH),
            b'(' => self.op(1, L_PAREN),
            b')' => self.op(1, R_PAREN),
            b'{' => self.op(1, L_CURLY),
            b'}' => self.op(1, R_CURLY),
            b']' => self.op(1, R_BRACK),
            b';' => self.op(1, SEMICOLON),
            b',' => self.op(1, COMMA),
            b'?' => self.op(1, QUESTION),
            b'"' | b'\'' => {
                self.read_string(start, c);
                STRING
            }
            b'`' => {
                self.read_string(start, c);
                HASH_STRING
            }
            b'.' => match (self.peek(1), self.peek(2)) {
                (b'.', b'.') => self.op(3, DOT3),
                (b'.', _) => self.op(2, DOT2),
                (d, _) if d.is_ascii_digit() => {
                    self.read_number(start);
                    NUMBER
                }
                _ => self.op(1, DOT),
            },
            b'0'..=b'9' => {
                self.read_number(start);
                NUMBER
            }
            c if c.is_ascii_alphabetic() || c == b'_' || c >= 0x80 => {
                // Non-ASCII characters make a name with an error, as LuaLS reads
                // them (`UNICODE_NAME`, off with `runtime.unicodeName`).
                while matches!(self.peek(0), b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | 0x80..=0xff) {
                    self.pos += 1;
                }
                // Whole characters: the source is UTF-8 and every byte >= 0x80 was taken.
                let text = std::str::from_utf8(&self.src[start..self.pos]).unwrap_or("");
                if !text.is_ascii() {
                    self.error(start, "UNICODE_NAME", "names with non-ASCII characters are not standard Lua");
                    return NAME;
                }
                SyntaxKind::from_keyword(text).unwrap_or(NAME)
            }
            _ => {
                // Skip one whole UTF-8 character so token boundaries stay on char boundaries.
                self.pos += utf8_len(c);
                self.error(start, "UNKNOWN_SYMBOL", "unexpected symbol");
                ERROR_TOKEN
            }
        }
    }

    /// At `[`: if it opens a long bracket (`[[`, `[=[`, ...) return its level, without consuming.
    fn long_bracket_level(&self) -> Option<usize> {
        let mut i = 1;
        while self.peek(i) == b'=' {
            i += 1;
        }
        (self.peek(i) == b'[').then_some(i - 1)
    }

    fn read_long_bracket(&mut self, start: usize, level: usize, what: &str) {
        self.pos += level + 2;
        loop {
            if self.at_end() {
                self.error(start, "MISS_SYMBOL", format!("unfinished long {what}"));
                return;
            }
            if self.peek(0) == b']' {
                let mut i = 1;
                while i <= level && self.peek(i) == b'=' {
                    i += 1;
                }
                if i == level + 1 && self.peek(i) == b']' {
                    self.pos += level + 2;
                    return;
                }
            }
            self.pos += 1;
        }
    }

    fn read_string(&mut self, start: usize, quote: u8) {
        self.pos += 1;
        loop {
            match self.peek(0) {
                _ if self.at_end() => {
                    self.error(start, "MISS_SYMBOL", "unfinished string");
                    return;
                }
                b'\n' | b'\r' => {
                    self.error(start, "MISS_SYMBOL", "unfinished string");
                    return;
                }
                b'\\' => {
                    let esc_start = self.pos;
                    self.pos += 1;
                    match self.peek(0) {
                        b'n' | b't' | b'r' | b'a' | b'b' | b'f' | b'v' | b'\\' | b'"' | b'\'' | b'`' => self.pos += 1,
                        b'\r' => {
                            self.pos += 1;
                            if self.peek(0) == b'\n' {
                                self.pos += 1;
                            }
                        }
                        b'\n' => {
                            self.pos += 1;
                            if self.peek(0) == b'\r' {
                                self.pos += 1;
                            }
                        }
                        b'x' => {
                            self.pos += 1;
                            for _ in 0..2 {
                                if self.peek(0).is_ascii_hexdigit() {
                                    self.pos += 1;
                                } else {
                                    self.error(esc_start, "ERR_ESC", "hexadecimal digit expected");
                                    break;
                                }
                            }
                        }
                        b'z' => {
                            self.pos += 1;
                            while matches!(self.peek(0), b' ' | b'\t' | b'\r' | b'\n' | 0x0b | 0x0c) && !self.at_end() {
                                self.pos += 1;
                            }
                        }
                        b'u' => {
                            self.pos += 1;
                            if self.peek(0) != b'{' {
                                self.error(esc_start, "ERR_ESC", "missing '{' in \\u{xxxx}");
                                continue;
                            }
                            self.pos += 1;
                            let digits_start = self.pos;
                            while self.peek(0).is_ascii_hexdigit() {
                                self.pos += 1;
                            }
                            if self.pos == digits_start {
                                self.error(esc_start, "ERR_ESC", "hexadecimal digit expected");
                            }
                            if self.peek(0) == b'}' {
                                self.pos += 1;
                            } else {
                                self.error(esc_start, "ERR_ESC", "missing '}' in \\u{xxxx}");
                            }
                        }
                        b'0'..=b'9' => {
                            let mut value = 0u32;
                            for _ in 0..3 {
                                match self.peek(0) {
                                    d @ b'0'..=b'9' => {
                                        value = value * 10 + (d - b'0') as u32;
                                        self.pos += 1;
                                    }
                                    _ => break,
                                }
                            }
                            if value > 255 {
                                self.error(esc_start, "ERR_ESC", "decimal escape too large");
                            }
                        }
                        _ if self.at_end() => {}
                        c => {
                            self.pos += utf8_len(c);
                            self.error(esc_start, "ERR_ESC", "invalid escape sequence");
                        }
                    }
                }
                c if c == quote => {
                    self.pos += 1;
                    return;
                }
                c => self.pos += utf8_len(c),
            }
        }
    }

    /// Lua reads a numeral greedily (`read_numeral` in llex.c), then validates it.
    fn read_number(&mut self, start: usize) {
        let hex = self.peek(0) == b'0' && matches!(self.peek(1), b'x' | b'X');
        if hex {
            self.pos += 2;
        }
        let (e1, e2) = if hex { (b'p', b'P') } else { (b'e', b'E') };
        loop {
            let c = self.peek(0);
            if (c == e1 || c == e2) && matches!(self.peek(1), b'+' | b'-') {
                self.pos += 2;
            } else if c.is_ascii_alphanumeric() || c == b'.' || c == b'_' {
                self.pos += 1;
            } else {
                break;
            }
        }
        let text = std::str::from_utf8(&self.src[start..self.pos]).unwrap();
        if !valid_number(text) {
            self.error(start, "MALFORMED_NUMBER", "malformed number");
        }
    }
}

fn utf8_len(first: u8) -> usize {
    match first {
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        _ => 1,
    }
}

fn valid_number(text: &str) -> bool {
    let b = text.as_bytes();
    if b.len() > 2 && b[0] == b'0' && (b[1] == b'x' || b[1] == b'X') {
        let rest = &b[2..];
        let (mantissa, exp) = match rest.iter().position(|&c| c == b'p' || c == b'P') {
            Some(i) => (&rest[..i], Some(&rest[i + 1..])),
            None => (rest, None),
        };
        let mut dots = 0;
        let mut digits = 0;
        for &c in mantissa {
            if c == b'.' {
                dots += 1;
            } else if c.is_ascii_hexdigit() {
                digits += 1;
            } else {
                return false;
            }
        }
        if dots > 1 || digits == 0 {
            return false;
        }
        return exp.is_none_or(valid_exponent);
    }
    let (mantissa, exp) = match b.iter().position(|&c| c == b'e' || c == b'E') {
        Some(i) => (&b[..i], Some(&b[i + 1..])),
        None => (b, None),
    };
    let mut dots = 0;
    let mut digits = 0;
    for &c in mantissa {
        if c == b'.' {
            dots += 1;
        } else if c.is_ascii_digit() {
            digits += 1;
        } else {
            return false;
        }
    }
    dots <= 1 && digits > 0 && exp.is_none_or(valid_exponent)
}

fn valid_exponent(e: &[u8]) -> bool {
    let e = match e.first() {
        Some(b'+' | b'-') => &e[1..],
        _ => e,
    };
    !e.is_empty() && e.iter().all(u8::is_ascii_digit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<SyntaxKind> {
        let (tokens, errors) = tokenize(src);
        assert!(errors.is_empty(), "{errors:?}");
        tokens.into_iter().map(|t| t.kind).filter(|k| *k != WHITESPACE).collect()
    }

    #[test]
    fn compound_operators() {
        assert_eq!(
            kinds("a += 1 b -= 2 c *= 3 d /= 4 e <<= 5 f >>= 6 g &= 7 h |= 8 i ^= 9"),
            [
                NAME, PLUS_EQ, NUMBER, NAME, MINUS_EQ, NUMBER, NAME, STAR_EQ, NUMBER, NAME, SLASH_EQ, NUMBER, NAME,
                SHL_EQ, NUMBER, NAME, SHR_EQ, NUMBER, NAME, AMP_EQ, NUMBER, NAME, PIPE_EQ, NUMBER, NAME, CARET_EQ,
                NUMBER
            ]
        );
    }

    #[test]
    fn comments() {
        assert_eq!(
            kinds("--x\n--[[a\nb]] /* c\n */ --[==[ ]] ]==]"),
            [COMMENT, BLOCK_COMMENT, BLOCK_COMMENT, BLOCK_COMMENT]
        );
        assert_eq!(kinds("--[ not long\nx"), [COMMENT, NAME]);
    }

    #[test]
    fn strings_and_hashes() {
        assert_eq!(
            kinds(
                r#"'a\'b' "c\z
              d" [==[x]]y]==] `adder`"#
            ),
            [STRING, STRING, LONG_STRING, HASH_STRING]
        );
    }

    #[test]
    fn numbers() {
        for n in ["1", "1.5", ".5", "1e10", "1E-3", "0x1F", "0x1p4", "0xA.8P-1", "3.", "0x.1"] {
            assert_eq!(kinds(n), [NUMBER], "{n}");
        }
        assert_eq!(tokenize("1e").1.len(), 1);
        assert_eq!(tokenize("0xg").1.len(), 1);
        assert_eq!(tokenize("1..2").1.len(), 1);
    }

    #[test]
    fn safe_navigation_and_misc() {
        assert_eq!(
            kinds("a?.b?[c] ... .. :: //"),
            [NAME, QUESTION, DOT, NAME, QUESTION, L_BRACK, NAME, R_BRACK, DOT3, DOT2, COLON2, SLASH2]
        );
    }

    #[test]
    fn lossless_on_errors() {
        let src = "x = 'open\ny = ç @ [==";
        let (tokens, errors) = tokenize(src);
        let total: u32 = tokens.iter().map(|t| u32::from(t.len)).sum();
        assert_eq!(total as usize, src.len());
        assert!(!errors.is_empty());
    }
}
