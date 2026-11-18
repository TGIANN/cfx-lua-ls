//! Format options: EmmyLuaCodeStyle's `LuaStyle` (same names, defaults and
//! `.editorconfig` / `Lua.format.defaultConfig` spellings).

use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndentStyle {
    Tab,
    Space,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuoteStyle {
    None,
    Single,
    Double,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallArgParentheses {
    Keep,
    Remove,
    RemoveStringOnly,
    RemoveTableOnly,
    Always,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContinuousAlign {
    None,
    WhenExtraSpace,
    Always,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableSeparatorStyle {
    None,
    Comma,
    Semicolon,
    OnlyKvColon,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrailingTableSeparator {
    Keep,
    Never,
    Smart,
    Always,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineSpaceType {
    Fixed,
    Keep,
    Max,
    Min,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineSpace {
    pub kind: LineSpaceType,
    pub space: usize,
}

impl LineSpace {
    pub const fn fixed(space: usize) -> LineSpace {
        LineSpace { kind: LineSpaceType::Fixed, space }
    }

    pub const fn keep() -> LineSpace {
        LineSpace { kind: LineSpaceType::Keep, space: 0 }
    }

    pub const fn new(kind: LineSpaceType, space: usize) -> LineSpace {
        LineSpace { kind, space }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionSingleArgSpace {
    None,
    Always,
    Keep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlignArrayTable {
    None,
    Normal,
    ContainCurly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlignChainExpr {
    None,
    Always,
    OnlyCallStmt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndStmtWithSemicolon {
    Keep,
    ReplaceWithNewline,
    Always,
    SameLine,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakTableList {
    Never,
    Smart,
    Lazy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpaceAroundStyle {
    None,
    Always,
    NoSpaceAsym,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpaceBeforeInlineCommentStyle {
    Fixed,
    Keep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpaceBeforeInlineComment {
    pub style: SpaceBeforeInlineCommentStyle,
    pub space: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndOfLine {
    Lf,
    CrLf,
    Cr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContinuationIndent {
    pub in_expr: usize,
    pub before_block: usize,
    pub in_table: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpaceBeforeSingleArg {
    pub table: FunctionSingleArgSpace,
    pub string: FunctionSingleArgSpace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpaceAroundMath {
    pub exponent: bool,
    pub other: bool,
}

/// `LuaStyle`. Defaults are EmmyLuaCodeStyle's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Style {
    // [basic]
    pub indent_style: IndentStyle,
    pub indent_size: usize,
    pub tab_width: usize,
    pub quote_style: QuoteStyle,
    pub continuation_indent: ContinuationIndent,
    pub table_separator_style: TableSeparatorStyle,
    pub max_line_length: usize,
    pub end_of_line: EndOfLine,
    pub trailing_table_separator: TrailingTableSeparator,
    pub call_arg_parentheses: CallArgParentheses,
    pub detect_end_of_line: bool,
    pub insert_final_newline: bool,
    // [space]
    pub space_around_table_field_list: bool,
    pub space_before_attribute: bool,
    pub space_before_function_open_parenthesis: bool,
    pub space_before_function_call_open_parenthesis: bool,
    pub space_before_closure_open_parenthesis: bool,
    pub space_before_function_call_single_arg: SpaceBeforeSingleArg,
    pub space_before_open_square_bracket: bool,
    pub space_inside_function_call_parentheses: bool,
    pub space_inside_function_param_list_parentheses: bool,
    pub space_inside_square_brackets: bool,
    pub space_around_table_append_operator: bool,
    pub ignore_spaces_inside_function_call: bool,
    pub space_before_inline_comment: SpaceBeforeInlineComment,
    pub space_after_comment_dash: bool,
    // [operator space]
    pub space_around_math_operator: SpaceAroundMath,
    pub space_after_comma: bool,
    pub space_after_comma_in_for_statement: bool,
    pub space_around_concat_operator: SpaceAroundStyle,
    pub space_around_logical_operator: bool,
    pub space_around_assign_operator: SpaceAroundStyle,
    // [align]
    pub align_call_args: bool,
    pub align_function_params: bool,
    pub align_continuous_assign_statement: ContinuousAlign,
    pub align_continuous_rect_table_field: ContinuousAlign,
    pub align_continuous_line_space: usize,
    pub align_if_branch: bool,
    pub align_continuous_similar_call_args: bool,
    pub align_continuous_inline_comment: bool,
    pub align_array_table: AlignArrayTable,
    pub align_chain_expr: AlignChainExpr,
    // [indent]
    pub never_indent_before_if_condition: bool,
    pub never_indent_comment_on_if_branch: bool,
    pub keep_indents_on_empty_lines: bool,
    pub allow_non_indented_comments: bool,
    // [line space]
    pub line_space_after_if_statement: LineSpace,
    pub line_space_after_do_statement: LineSpace,
    pub line_space_after_while_statement: LineSpace,
    pub line_space_after_repeat_statement: LineSpace,
    pub line_space_after_for_statement: LineSpace,
    pub line_space_after_local_or_assign_statement: LineSpace,
    pub line_space_after_function_statement: LineSpace,
    pub line_space_after_expression_statement: LineSpace,
    pub line_space_after_comment: LineSpace,
    pub line_space_around_block: LineSpace,
    // [line break]
    pub break_all_list_when_line_exceed: bool,
    pub auto_collapse_lines: bool,
    pub break_before_braces: bool,
    pub break_table_list: BreakTableList,
    // [preference]
    pub ignore_space_after_colon: bool,
    pub remove_call_expression_list_finish_comma: bool,
    pub end_statement_with_semicolon: EndStmtWithSemicolon,
    pub remove_leading_zero_in_float: bool,
}

impl Default for Style {
    fn default() -> Style {
        Style {
            indent_style: IndentStyle::Space,
            indent_size: 4,
            tab_width: 4,
            quote_style: QuoteStyle::None,
            continuation_indent: ContinuationIndent { in_expr: 4, before_block: 4, in_table: 4 },
            table_separator_style: TableSeparatorStyle::Comma,
            max_line_length: 120,
            end_of_line: platform_eol(),
            trailing_table_separator: TrailingTableSeparator::Keep,
            call_arg_parentheses: CallArgParentheses::Keep,
            detect_end_of_line: true,
            insert_final_newline: true,
            space_around_table_field_list: true,
            space_before_attribute: true,
            space_before_function_open_parenthesis: false,
            space_before_function_call_open_parenthesis: false,
            space_before_closure_open_parenthesis: false,
            space_before_function_call_single_arg: SpaceBeforeSingleArg {
                table: FunctionSingleArgSpace::Always,
                string: FunctionSingleArgSpace::Always,
            },
            space_before_open_square_bracket: false,
            space_inside_function_call_parentheses: false,
            space_inside_function_param_list_parentheses: false,
            space_inside_square_brackets: false,
            space_around_table_append_operator: false,
            ignore_spaces_inside_function_call: false,
            space_before_inline_comment: SpaceBeforeInlineComment {
                style: SpaceBeforeInlineCommentStyle::Fixed,
                space: 1,
            },
            space_after_comment_dash: false,
            space_around_math_operator: SpaceAroundMath { exponent: true, other: true },
            space_after_comma: true,
            space_after_comma_in_for_statement: true,
            space_around_concat_operator: SpaceAroundStyle::Always,
            space_around_logical_operator: true,
            space_around_assign_operator: SpaceAroundStyle::Always,
            align_call_args: false,
            align_function_params: true,
            align_continuous_assign_statement: ContinuousAlign::WhenExtraSpace,
            align_continuous_rect_table_field: ContinuousAlign::WhenExtraSpace,
            align_continuous_line_space: 2,
            align_if_branch: false,
            align_continuous_similar_call_args: false,
            align_continuous_inline_comment: true,
            align_array_table: AlignArrayTable::Normal,
            align_chain_expr: AlignChainExpr::None,
            never_indent_before_if_condition: false,
            never_indent_comment_on_if_branch: false,
            keep_indents_on_empty_lines: false,
            allow_non_indented_comments: false,
            line_space_after_if_statement: LineSpace::keep(),
            line_space_after_do_statement: LineSpace::keep(),
            line_space_after_while_statement: LineSpace::keep(),
            line_space_after_repeat_statement: LineSpace::keep(),
            line_space_after_for_statement: LineSpace::keep(),
            line_space_after_local_or_assign_statement: LineSpace::keep(),
            line_space_after_function_statement: LineSpace::fixed(2),
            line_space_after_expression_statement: LineSpace::keep(),
            line_space_after_comment: LineSpace::keep(),
            line_space_around_block: LineSpace::fixed(1),
            break_all_list_when_line_exceed: false,
            auto_collapse_lines: false,
            break_before_braces: false,
            break_table_list: BreakTableList::Smart,
            ignore_space_after_colon: false,
            remove_call_expression_list_finish_comma: false,
            end_statement_with_semicolon: EndStmtWithSemicolon::Keep,
            remove_leading_zero_in_float: false,
        }
    }
}

/// `end_of_line = auto`: CRLF on Windows, LF elsewhere (the C++ default). It only
/// shows when `detect_end_of_line` is off or the file has no line break.
fn platform_eol() -> EndOfLine {
    if cfg!(windows) { EndOfLine::CrLf } else { EndOfLine::Lf }
}

fn number(v: &str) -> Option<usize> {
    (!v.is_empty() && v.bytes().all(|b| b.is_ascii_digit())).then(|| v.parse().ok()).flatten()
}

fn boolean(v: &str) -> Option<bool> {
    match v {
        "true" | "always" => Some(true),
        "false" | "none" => Some(false),
        _ => None,
    }
}

fn single_arg(v: &str) -> Option<FunctionSingleArgSpace> {
    match v {
        "true" | "always" => Some(FunctionSingleArgSpace::Always),
        "false" | "none" => Some(FunctionSingleArgSpace::None),
        "keep" => Some(FunctionSingleArgSpace::Keep),
        _ => None,
    }
}

fn space_around(v: &str) -> Option<SpaceAroundStyle> {
    match v {
        "true" | "always" => Some(SpaceAroundStyle::Always),
        "false" | "none" => Some(SpaceAroundStyle::None),
        "no_space_asym" => Some(SpaceAroundStyle::NoSpaceAsym),
        _ => None,
    }
}

fn continuous_align(v: &str) -> Option<ContinuousAlign> {
    match v {
        "true" | "when_extra_space" => Some(ContinuousAlign::WhenExtraSpace),
        "always" => Some(ContinuousAlign::Always),
        "false" | "none" => Some(ContinuousAlign::None),
        _ => None,
    }
}

/// `keep`, `fixed(n)`, `min(n)`, `max(n)` (`FunctionOption`).
fn line_space(v: &str) -> Option<LineSpace> {
    let v = v.trim();
    let (key, param) = match v.find('(') {
        Some(open) => {
            let close = v[open..].find(')').map_or(v.len(), |c| c + open);
            (v[..open].trim(), Some(v[open + 1..close].split(',').next().unwrap_or("").trim()))
        }
        None => (v, None),
    };
    let n = || param.and_then(number);
    match key {
        "keep" => Some(LineSpace::keep()),
        "fixed" => n().map(LineSpace::fixed),
        "min" => n().map(|n| LineSpace::new(LineSpaceType::Min, n)),
        "max" => n().map(|n| LineSpace::new(LineSpaceType::Max, n)),
        _ => None,
    }
}

impl Style {
    /// Applies the options in `map` (`LuaStyle::Parse`); unknown keys and values
    /// are ignored.
    pub fn apply(&mut self, map: &HashMap<String, String>) {
        let get = |k: &str| map.get(k).map(String::as_str).filter(|v| !v.is_empty());
        macro_rules! set_bool {
            ($($f:ident),* $(,)?) => {$(
                if let Some(b) = get(stringify!($f)).and_then(boolean) {
                    self.$f = b;
                }
            )*};
        }
        macro_rules! set_num {
            ($($f:ident),* $(,)?) => {$(
                if let Some(n) = get(stringify!($f)).and_then(number) {
                    self.$f = n;
                }
            )*};
        }
        match get("indent_style") {
            Some("tab") => self.indent_style = IndentStyle::Tab,
            Some("space") => self.indent_style = IndentStyle::Space,
            _ => {}
        }
        set_num!(indent_size, tab_width);
        for key in ["quote_style", "quote_type"] {
            match get(key) {
                Some("single") => self.quote_style = QuoteStyle::Single,
                Some("double") => self.quote_style = QuoteStyle::Double,
                _ => {}
            }
        }
        match get("table_separator_style") {
            Some("none") => self.table_separator_style = TableSeparatorStyle::None,
            Some("comma") => self.table_separator_style = TableSeparatorStyle::Comma,
            Some("semicolon") => self.table_separator_style = TableSeparatorStyle::Semicolon,
            Some("only_kv_colon") => self.table_separator_style = TableSeparatorStyle::OnlyKvColon,
            _ => {}
        }
        match get("max_line_length") {
            Some("unset") => self.max_line_length = 998,
            Some(v) => {
                if let Some(n) = number(v) {
                    self.max_line_length = n;
                }
            }
            None => {}
        }
        match get("end_of_line") {
            Some("crlf") => self.end_of_line = EndOfLine::CrLf,
            Some("lf") => self.end_of_line = EndOfLine::Lf,
            Some("cr") => self.end_of_line = EndOfLine::Cr,
            Some("auto" | "unset") => self.end_of_line = platform_eol(),
            _ => {}
        }
        match get("trailing_table_separator") {
            Some("keep") => self.trailing_table_separator = TrailingTableSeparator::Keep,
            Some("never") => self.trailing_table_separator = TrailingTableSeparator::Never,
            Some("always") => self.trailing_table_separator = TrailingTableSeparator::Always,
            Some("smart") => self.trailing_table_separator = TrailingTableSeparator::Smart,
            _ => {}
        }
        match get("call_arg_parentheses") {
            Some("keep") => self.call_arg_parentheses = CallArgParentheses::Keep,
            Some("remove") => self.call_arg_parentheses = CallArgParentheses::Remove,
            Some("remove_string_only") => self.call_arg_parentheses = CallArgParentheses::RemoveStringOnly,
            Some("remove_table_only") => self.call_arg_parentheses = CallArgParentheses::RemoveTableOnly,
            Some("always") => self.call_arg_parentheses = CallArgParentheses::Always,
            _ => {}
        }
        if let Some(n) = get("continuation_indent").and_then(number) {
            self.continuation_indent = ContinuationIndent { in_expr: n, before_block: n, in_table: n };
        }
        if let Some(n) = get("continuation_indent.before_block").and_then(number) {
            self.continuation_indent.before_block = n;
        }
        if let Some(n) = get("continuation_indent.in_expr").and_then(number) {
            self.continuation_indent.in_expr = n;
        }
        if let Some(n) = get("continuation_indent.in_table").and_then(number) {
            self.continuation_indent.in_table = n;
        }
        set_bool!(
            detect_end_of_line,
            insert_final_newline,
            space_around_table_field_list,
            space_before_attribute,
            space_before_function_open_parenthesis,
            space_before_function_call_open_parenthesis,
            space_before_closure_open_parenthesis,
        );
        match get("space_before_function_call_single_arg") {
            Some("true" | "always") => {
                self.space_before_function_call_single_arg.table = FunctionSingleArgSpace::Always;
                self.space_before_function_call_single_arg.string = FunctionSingleArgSpace::Always;
            }
            Some("only_string") => {
                self.space_before_function_call_single_arg.string = FunctionSingleArgSpace::Always;
                self.space_before_function_call_single_arg.table = FunctionSingleArgSpace::None;
            }
            Some("only_table") => {
                self.space_before_function_call_single_arg.string = FunctionSingleArgSpace::None;
                self.space_before_function_call_single_arg.table = FunctionSingleArgSpace::Always;
            }
            Some("false" | "none") => {
                self.space_before_function_call_single_arg.table = FunctionSingleArgSpace::None;
                self.space_before_function_call_single_arg.string = FunctionSingleArgSpace::None;
            }
            _ => {}
        }
        if let Some(s) = get("space_before_function_call_single_arg.string").and_then(single_arg) {
            self.space_before_function_call_single_arg.string = s;
        }
        if let Some(s) = get("space_before_function_call_single_arg.table").and_then(single_arg) {
            self.space_before_function_call_single_arg.table = s;
        }
        set_bool!(
            space_before_open_square_bracket,
            space_inside_function_call_parentheses,
            space_inside_function_param_list_parentheses,
            space_inside_square_brackets,
            space_around_table_append_operator,
            ignore_spaces_inside_function_call,
        );
        match get("space_before_inline_comment") {
            Some("keep") => {
                self.space_before_inline_comment =
                    SpaceBeforeInlineComment { style: SpaceBeforeInlineCommentStyle::Keep, space: 0 };
            }
            Some(v) => {
                if let Some(n) = number(v) {
                    self.space_before_inline_comment =
                        SpaceBeforeInlineComment { style: SpaceBeforeInlineCommentStyle::Fixed, space: n };
                }
            }
            None => {}
        }
        set_bool!(space_after_comment_dash);
        if let Some(b) = get("space_around_math_operator").and_then(boolean) {
            self.space_around_math_operator = SpaceAroundMath { exponent: b, other: b };
        }
        if let Some(b) = get("space_around_math_operator.exponent").and_then(boolean) {
            self.space_around_math_operator.exponent = b;
        }
        set_bool!(space_after_comma, space_after_comma_in_for_statement);
        if let Some(s) = get("space_around_concat_operator").and_then(space_around) {
            self.space_around_concat_operator = s;
        }
        set_bool!(space_around_logical_operator);
        if let Some(s) = get("space_around_assign_operator").and_then(space_around) {
            self.space_around_assign_operator = s;
        }
        set_bool!(align_call_args, align_function_params);
        if let Some(a) = get("align_continuous_assign_statement").and_then(continuous_align) {
            self.align_continuous_assign_statement = a;
        }
        if let Some(a) = get("align_continuous_rect_table_field").and_then(continuous_align) {
            self.align_continuous_rect_table_field = a;
        }
        set_num!(align_continuous_line_space);
        set_bool!(align_if_branch);
        match get("align_array_table") {
            Some("true" | "normal") => self.align_array_table = AlignArrayTable::Normal,
            Some("contain_curly") => self.align_array_table = AlignArrayTable::ContainCurly,
            Some("false" | "none") => self.align_array_table = AlignArrayTable::None,
            _ => {}
        }
        match get("align_chain_expr") {
            Some("none") => self.align_chain_expr = AlignChainExpr::None,
            Some("always") => self.align_chain_expr = AlignChainExpr::Always,
            Some("only_call_stmt") => self.align_chain_expr = AlignChainExpr::OnlyCallStmt,
            _ => {}
        }
        set_bool!(align_continuous_similar_call_args, align_continuous_inline_comment, never_indent_before_if_condition);
        if self.align_if_branch {
            self.never_indent_before_if_condition = true;
        }
        set_bool!(never_indent_comment_on_if_branch, keep_indents_on_empty_lines, allow_non_indented_comments);
        macro_rules! set_line_space {
            ($($f:ident),* $(,)?) => {$(
                if let Some(l) = map.get(stringify!($f)).and_then(|v| line_space(v)) {
                    self.$f = l;
                }
            )*};
        }
        set_line_space!(
            line_space_after_if_statement,
            line_space_after_do_statement,
            line_space_after_while_statement,
            line_space_after_repeat_statement,
            line_space_after_for_statement,
            line_space_after_local_or_assign_statement,
            line_space_after_function_statement,
            line_space_after_expression_statement,
            line_space_after_comment,
            line_space_around_block,
        );
        set_bool!(break_all_list_when_line_exceed, auto_collapse_lines, break_before_braces);
        match get("break_table_list") {
            Some("never") => self.break_table_list = BreakTableList::Never,
            Some("smart") => self.break_table_list = BreakTableList::Smart,
            Some("lazy") => self.break_table_list = BreakTableList::Lazy,
            _ => {}
        }
        set_bool!(ignore_space_after_colon, remove_call_expression_list_finish_comma);
        match get("end_statement_with_semicolon") {
            Some("keep") => self.end_statement_with_semicolon = EndStmtWithSemicolon::Keep,
            Some("always") => self.end_statement_with_semicolon = EndStmtWithSemicolon::Always,
            Some("replace_with_newline") => {
                self.end_statement_with_semicolon = EndStmtWithSemicolon::ReplaceWithNewline;
            }
            Some("same_line") => self.end_statement_with_semicolon = EndStmtWithSemicolon::SameLine,
            Some("never") => self.end_statement_with_semicolon = EndStmtWithSemicolon::Never,
            _ => {}
        }
        set_bool!(remove_leading_zero_in_float);
    }

    /// The editor's `tabSize` / `insertSpaces` on top (`CalculateTempStyle`): they
    /// win over `.editorconfig`, as in LuaLS.
    pub fn with_editor_options(mut self, tab_size: Option<usize>, insert_spaces: Option<bool>) -> Style {
        if let Some(spaces) = insert_spaces {
            self.indent_style = if spaces { IndentStyle::Space } else { IndentStyle::Tab };
        }
        if let Some(size) = tab_size {
            match self.indent_style {
                IndentStyle::Tab => self.tab_width = size,
                IndentStyle::Space => self.indent_size = size,
            }
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|&(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn options_parse_like_lua_style() {
        let mut s = Style::default();
        s.apply(&map(&[
            ("indent_style", "tab"),
            ("quote_style", "single"),
            ("max_line_length", "unset"),
            ("space_before_function_call_single_arg", "only_table"),
            ("line_space_after_function_statement", "max(1)"),
            ("line_space_around_block", "bogus(1)"),
            ("align_if_branch", "true"),
            ("indent_size", "x"),
        ]));
        assert_eq!(s.indent_style, IndentStyle::Tab);
        assert_eq!(s.quote_style, QuoteStyle::Single);
        assert_eq!(s.max_line_length, 998);
        assert_eq!(s.space_before_function_call_single_arg.string, FunctionSingleArgSpace::None);
        assert_eq!(s.line_space_after_function_statement, LineSpace::new(LineSpaceType::Max, 1));
        assert_eq!(s.line_space_around_block, LineSpace::fixed(1));
        assert!(s.never_indent_before_if_condition);
        assert_eq!(s.indent_size, 4);
    }
}
