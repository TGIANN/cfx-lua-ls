//! Every token and node kind of the CfxLua syntax tree.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
#[allow(non_camel_case_types)]
pub enum SyntaxKind {
    // ---- trivia ----
    WHITESPACE = 0,
    /// `-- ...` up to the end of the line (includes `---@` doc lines).
    COMMENT,
    /// `--[[ ... ]]` and CfxLua `/* ... */`.
    BLOCK_COMMENT,
    /// `#!...` on the first line.
    SHEBANG,

    // ---- literals and names ----
    NAME,
    NUMBER,
    /// `"..."` or `'...'`.
    STRING,
    /// `[[...]]`, `[==[...]==]`.
    LONG_STRING,
    /// CfxLua compile-time Jenkins hash: `` `adder` ``.
    HASH_STRING,

    // ---- keywords ----
    AND_KW,
    BREAK_KW,
    DO_KW,
    ELSE_KW,
    ELSEIF_KW,
    END_KW,
    FALSE_KW,
    FOR_KW,
    FUNCTION_KW,
    GOTO_KW,
    IF_KW,
    IN_KW,
    LOCAL_KW,
    NIL_KW,
    NOT_KW,
    OR_KW,
    REPEAT_KW,
    RETURN_KW,
    THEN_KW,
    TRUE_KW,
    UNTIL_KW,
    WHILE_KW,

    // ---- punctuation ----
    PLUS,      // +
    MINUS,     // -
    STAR,      // *
    SLASH,     // /
    SLASH2,    // //
    PERCENT,   // %
    CARET,     // ^
    HASH,      // #
    AMP,       // &
    TILDE,     // ~
    PIPE,      // |
    SHL,       // <<
    SHR,       // >>
    EQ2,       // ==
    NEQ,       // ~=
    LTEQ,      // <=
    GTEQ,      // >=
    LT,        // <
    GT,        // >
    EQ,        // =
    L_PAREN,   // (
    R_PAREN,   // )
    L_CURLY,   // {
    R_CURLY,   // }
    L_BRACK,   // [
    R_BRACK,   // ]
    COLON2,    // ::
    SEMICOLON, // ;
    COLON,     // :
    COMMA,     // ,
    DOT,       // .
    DOT2,      // ..
    DOT3,      // ...
    QUESTION,  // ? (CfxLua safe navigation, always followed by `.` or `[`)
    // CfxLua compound assignment
    PLUS_EQ,  // +=
    MINUS_EQ, // -=
    STAR_EQ,  // *=
    SLASH_EQ, // /=
    SHL_EQ,   // <<=
    SHR_EQ,   // >>=
    AMP_EQ,   // &=
    PIPE_EQ,  // |=
    CARET_EQ, // ^=  (bitwise xor in CfxLua, not power)

    /// A character the lexer could not use.
    ERROR_TOKEN,
    EOF,

    // ---- nodes ----
    SOURCE_FILE,
    BLOCK,

    // statements
    EMPTY_STAT,
    LOCAL_STAT,
    LOCAL_FUNCTION_STAT,
    FUNCTION_STAT,
    ASSIGN_STAT,
    COMPOUND_ASSIGN_STAT,
    CALL_STAT,
    DO_STAT,
    WHILE_STAT,
    REPEAT_STAT,
    IF_STAT,
    ELSEIF_CLAUSE,
    ELSE_CLAUSE,
    NUMERIC_FOR_STAT,
    GENERIC_FOR_STAT,
    RETURN_STAT,
    BREAK_STAT,
    GOTO_STAT,
    LABEL_STAT,

    // parts
    LOCAL_NAME,
    ATTRIB,
    VAR_LIST,
    EXPR_LIST,
    FUNC_NAME,
    FUNC_BODY,
    PARAM_LIST,
    PARAM,
    ARG_LIST,

    // expressions
    NAME_EXPR,
    LITERAL_EXPR,
    VARARG_EXPR,
    PAREN_EXPR,
    FUNCTION_EXPR,
    TABLE_EXPR,
    INDEX_EXPR,
    CALL_EXPR,
    METHOD_CALL_EXPR,
    BINARY_EXPR,
    UNARY_EXPR,

    // table fields
    /// `expr`
    FIELD_POS,
    /// `name = expr`
    FIELD_NAMED,
    /// `[expr] = expr`
    FIELD_INDEXED,
    /// CfxLua set constructor: `.name` or `.name = expr`
    FIELD_SET,

    ERROR,

    #[doc(hidden)]
    __LAST,
}

use SyntaxKind::*;

impl SyntaxKind {
    pub fn is_trivia(self) -> bool {
        matches!(self, WHITESPACE | COMMENT | BLOCK_COMMENT | SHEBANG)
    }

    pub fn is_keyword(self) -> bool {
        (AND_KW as u16..=WHILE_KW as u16).contains(&(self as u16))
    }

    pub fn is_compound_assign(self) -> bool {
        (PLUS_EQ as u16..=CARET_EQ as u16).contains(&(self as u16))
    }

    /// Statements and table fields: the comments directly above one go inside its
    /// node. No other node starts with trivia, except `BLOCK` and `SOURCE_FILE`.
    pub fn attaches_comments(self) -> bool {
        matches!(
            self,
            EMPTY_STAT
                | LOCAL_STAT
                | LOCAL_FUNCTION_STAT
                | FUNCTION_STAT
                | ASSIGN_STAT
                | COMPOUND_ASSIGN_STAT
                | CALL_STAT
                | DO_STAT
                | WHILE_STAT
                | REPEAT_STAT
                | IF_STAT
                | NUMERIC_FOR_STAT
                | GENERIC_FOR_STAT
                | RETURN_STAT
                | BREAK_STAT
                | GOTO_STAT
                | LABEL_STAT
                | FIELD_POS
                | FIELD_NAMED
                | FIELD_INDEXED
                | FIELD_SET
        )
    }

    pub fn from_keyword(text: &str) -> Option<SyntaxKind> {
        Some(match text {
            "and" => AND_KW,
            "break" => BREAK_KW,
            "do" => DO_KW,
            "else" => ELSE_KW,
            "elseif" => ELSEIF_KW,
            "end" => END_KW,
            "false" => FALSE_KW,
            "for" => FOR_KW,
            "function" => FUNCTION_KW,
            "goto" => GOTO_KW,
            "if" => IF_KW,
            "in" => IN_KW,
            "local" => LOCAL_KW,
            "nil" => NIL_KW,
            "not" => NOT_KW,
            "or" => OR_KW,
            "repeat" => REPEAT_KW,
            "return" => RETURN_KW,
            "then" => THEN_KW,
            "true" => TRUE_KW,
            "until" => UNTIL_KW,
            "while" => WHILE_KW,
            _ => return None,
        })
    }

    /// Human readable form used in "expected X" errors.
    pub fn describe(self) -> &'static str {
        match self {
            NAME => "a name",
            NUMBER => "a number",
            STRING | LONG_STRING => "a string",
            HASH_STRING => "a hash string",
            AND_KW => "'and'",
            BREAK_KW => "'break'",
            DO_KW => "'do'",
            ELSE_KW => "'else'",
            ELSEIF_KW => "'elseif'",
            END_KW => "'end'",
            FALSE_KW => "'false'",
            FOR_KW => "'for'",
            FUNCTION_KW => "'function'",
            GOTO_KW => "'goto'",
            IF_KW => "'if'",
            IN_KW => "'in'",
            LOCAL_KW => "'local'",
            NIL_KW => "'nil'",
            NOT_KW => "'not'",
            OR_KW => "'or'",
            REPEAT_KW => "'repeat'",
            RETURN_KW => "'return'",
            THEN_KW => "'then'",
            TRUE_KW => "'true'",
            UNTIL_KW => "'until'",
            WHILE_KW => "'while'",
            EQ => "'='",
            L_PAREN => "'('",
            R_PAREN => "')'",
            L_CURLY => "'{'",
            R_CURLY => "'}'",
            L_BRACK => "'['",
            R_BRACK => "']'",
            COLON2 => "'::'",
            COMMA => "','",
            GT => "'>'",
            EOF => "end of file",
            _ => "a token",
        }
    }
}

impl From<SyntaxKind> for rowan::SyntaxKind {
    fn from(kind: SyntaxKind) -> Self {
        rowan::SyntaxKind(kind as u16)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CfxLua {}

impl rowan::Language for CfxLua {
    type Kind = SyntaxKind;

    fn kind_from_raw(raw: rowan::SyntaxKind) -> SyntaxKind {
        assert!(raw.0 < __LAST as u16);
        // SAFETY: SyntaxKind is repr(u16) and every value below __LAST is a variant.
        unsafe { std::mem::transmute::<u16, SyntaxKind>(raw.0) }
    }

    fn kind_to_raw(kind: SyntaxKind) -> rowan::SyntaxKind {
        kind.into()
    }
}

pub type SyntaxNode = rowan::SyntaxNode<CfxLua>;
pub type SyntaxToken = rowan::SyntaxToken<CfxLua>;
pub type SyntaxElement = rowan::SyntaxElement<CfxLua>;
