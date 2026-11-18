//! EmmyLuaCodeStyle's own unit tests (`Test/src/*_unitest.cpp`), extracted by
//! `tools/extract_codestyle_tests.py`. Compared as C++ compares them: both sides
//! with surrounding whitespace trimmed.

use cfx_lua_fmt::style::*;
use serde_json::Value;

/// Cases we leave out, with the reason. Their parser accepts these inputs; ours
/// reports a syntax error, and a file with errors is never formatted.
const SKIP: &[(&str, &str)] = &[
    ("FormatByStyleOption.remove_call_expression_list_finish_comma", "trailing comma in call arguments"),
    ("FormatByStyleOption.align_chain_expr", "`return` before the end of a block"),
    ("Format.feature_107_ignore_unicode_align", "non-ASCII names and table fields without separators"),
];

fn variant(v: &str) -> &str {
    v.rsplit("::").next().unwrap_or(v)
}

fn boolean(v: &str) -> Result<bool, String> {
    v.parse().map_err(|_| format!("not a bool: {v}"))
}

fn number(v: &str) -> Result<usize, String> {
    v.parse().map_err(|_| format!("not a number: {v}"))
}

fn single_arg(v: &str) -> Result<FunctionSingleArgSpace, String> {
    Ok(match variant(v) {
        "None" => FunctionSingleArgSpace::None,
        "Always" => FunctionSingleArgSpace::Always,
        "Keep" => FunctionSingleArgSpace::Keep,
        _ => return Err(format!("FunctionSingleArgSpace: {v}")),
    })
}

fn continuous(v: &str) -> Result<ContinuousAlign, String> {
    Ok(match variant(v) {
        "None" => ContinuousAlign::None,
        "WhenExtraSpace" => ContinuousAlign::WhenExtraSpace,
        "Always" => ContinuousAlign::Always,
        _ => return Err(format!("ContinuousAlign: {v}")),
    })
}

fn space_around(v: &str) -> Result<SpaceAroundStyle, String> {
    Ok(match variant(v) {
        "None" => SpaceAroundStyle::None,
        "Always" => SpaceAroundStyle::Always,
        "NoSpaceAsym" => SpaceAroundStyle::NoSpaceAsym,
        _ => return Err(format!("SpaceAroundStyle: {v}")),
    })
}

/// One `style.<key> = <value>;` line of the C++ tests.
fn set(s: &mut Style, key: &str, v: &str) -> Result<(), String> {
    macro_rules! bools {
        ($($f:ident),* $(,)?) => {
            match key {
                $(stringify!($f) => { s.$f = boolean(v)?; return Ok(()); })*
                _ => {}
            }
        };
    }
    bools!(
        align_call_args,
        align_continuous_inline_comment,
        align_continuous_similar_call_args,
        align_function_params,
        align_if_branch,
        allow_non_indented_comments,
        auto_collapse_lines,
        break_all_list_when_line_exceed,
        break_before_braces,
        ignore_space_after_colon,
        ignore_spaces_inside_function_call,
        keep_indents_on_empty_lines,
        never_indent_before_if_condition,
        never_indent_comment_on_if_branch,
        remove_call_expression_list_finish_comma,
        space_after_comma,
        space_after_comma_in_for_statement,
        space_after_comment_dash,
        space_around_logical_operator,
        space_around_table_append_operator,
        space_around_table_field_list,
        space_before_attribute,
        space_before_closure_open_parenthesis,
        space_before_function_call_open_parenthesis,
        space_before_function_open_parenthesis,
        space_before_open_square_bracket,
        space_inside_function_call_parentheses,
        space_inside_function_param_list_parentheses,
        space_inside_square_brackets,
    );
    match key {
        "indent_size" => s.indent_size = number(v)?,
        "max_line_length" => s.max_line_length = number(v)?,
        "indent_style" => {
            s.indent_style = match variant(v) {
                "Space" => IndentStyle::Space,
                "Tab" => IndentStyle::Tab,
                _ => return Err(format!("IndentStyle: {v}")),
            }
        }
        "quote_style" => {
            s.quote_style = match variant(v) {
                "None" => QuoteStyle::None,
                "Single" => QuoteStyle::Single,
                "Double" => QuoteStyle::Double,
                _ => return Err(format!("QuoteStyle: {v}")),
            }
        }
        "align_array_table" => {
            s.align_array_table = match variant(v) {
                "None" => AlignArrayTable::None,
                "Normal" => AlignArrayTable::Normal,
                "ContainCurly" => AlignArrayTable::ContainCurly,
                _ => return Err(format!("AlignArrayTable: {v}")),
            }
        }
        "align_chain_expr" => {
            s.align_chain_expr = match variant(v) {
                "None" => AlignChainExpr::None,
                "Always" => AlignChainExpr::Always,
                "OnlyCallStmt" => AlignChainExpr::OnlyCallStmt,
                _ => return Err(format!("AlignChainExpr: {v}")),
            }
        }
        "align_continuous_assign_statement" => s.align_continuous_assign_statement = continuous(v)?,
        "align_continuous_rect_table_field" => s.align_continuous_rect_table_field = continuous(v)?,
        "call_arg_parentheses" => {
            s.call_arg_parentheses = match variant(v) {
                "Keep" => CallArgParentheses::Keep,
                "Remove" => CallArgParentheses::Remove,
                "RemoveStringOnly" => CallArgParentheses::RemoveStringOnly,
                "RemoveTableOnly" => CallArgParentheses::RemoveTableOnly,
                "Always" => CallArgParentheses::Always,
                _ => return Err(format!("CallArgParentheses: {v}")),
            }
        }
        "continuation_indent.SetAll" => {
            let n = number(v)?;
            s.continuation_indent = ContinuationIndent { in_expr: n, before_block: n, in_table: n };
        }
        "continuation_indent.before_block" => s.continuation_indent.before_block = number(v)?,
        "continuation_indent.in_expr" => s.continuation_indent.in_expr = number(v)?,
        "continuation_indent.in_table" => s.continuation_indent.in_table = number(v)?,
        "end_statement_with_semicolon" => {
            s.end_statement_with_semicolon = match variant(v) {
                "Keep" => EndStmtWithSemicolon::Keep,
                "Always" => EndStmtWithSemicolon::Always,
                "ReplaceWithNewline" => EndStmtWithSemicolon::ReplaceWithNewline,
                "SameLine" => EndStmtWithSemicolon::SameLine,
                "Never" => EndStmtWithSemicolon::Never,
                _ => return Err(format!("EndStmtWithSemicolon: {v}")),
            }
        }
        "line_space_around_block" => {
            s.line_space_around_block = match v {
                "LineSpace(LineSpaceType::Keep)" => LineSpace::keep(),
                "LineSpace(LineSpaceType::Max, 2)" => LineSpace::new(LineSpaceType::Max, 2),
                _ => return Err(format!("LineSpace: {v}")),
            }
        }
        "space_around_assign_operator" => s.space_around_assign_operator = space_around(v)?,
        "space_around_concat_operator" => s.space_around_concat_operator = space_around(v)?,
        "space_around_math_operator.SetAll" => {
            let b = boolean(v)?;
            s.space_around_math_operator = SpaceAroundMath { exponent: b, other: b };
        }
        "space_around_math_operator.exponent" => s.space_around_math_operator.exponent = boolean(v)?,
        "space_before_function_call_single_arg.SetAll" => {
            let a = single_arg(v)?;
            s.space_before_function_call_single_arg = SpaceBeforeSingleArg { table: a, string: a };
        }
        "space_before_function_call_single_arg.string" => s.space_before_function_call_single_arg.string = single_arg(v)?,
        "space_before_function_call_single_arg.table" => s.space_before_function_call_single_arg.table = single_arg(v)?,
        "space_before_inline_comment" => {
            s.space_before_inline_comment = match v {
                "SpaceBeforeInlineComment(0, SpaceBeforeInlineCommentStyle::Keep)" => {
                    SpaceBeforeInlineComment { style: SpaceBeforeInlineCommentStyle::Keep, space: 0 }
                }
                _ => {
                    let n = v
                        .strip_prefix("SpaceBeforeInlineComment(")
                        .and_then(|r| r.strip_suffix(')'))
                        .ok_or(format!("SpaceBeforeInlineComment: {v}"))?;
                    SpaceBeforeInlineComment { style: SpaceBeforeInlineCommentStyle::Fixed, space: number(n)? }
                }
            }
        }
        "table_separator_style" => {
            s.table_separator_style = match variant(v) {
                "None" => TableSeparatorStyle::None,
                "Comma" => TableSeparatorStyle::Comma,
                "Semicolon" => TableSeparatorStyle::Semicolon,
                "OnlyKVColon" => TableSeparatorStyle::OnlyKvColon,
                _ => return Err(format!("TableSeparatorStyle: {v}")),
            }
        }
        "trailing_table_separator" => {
            s.trailing_table_separator = match variant(v) {
                "Keep" => TrailingTableSeparator::Keep,
                "Never" => TrailingTableSeparator::Never,
                "Always" => TrailingTableSeparator::Always,
                "Smart" => TrailingTableSeparator::Smart,
                _ => return Err(format!("TrailingTableSeparator: {v}")),
            }
        }
        _ => return Err(format!("unknown style key {key}")),
    }
    Ok(())
}

/// `string_util::TrimSpace`.
fn trim(s: &str) -> &str {
    s.trim_matches(|c| matches!(c, '\n' | '\r' | ' ' | '\t' | '\x0b' | '\x0c'))
}

#[test]
fn emmylua_codestyle_cases() {
    let data = include_str!("data/codestyle_tests.json");
    let cases: Vec<Value> = serde_json::from_str(data).unwrap();
    let mut failed = Vec::new();
    let mut passed = 0;
    let mut skipped = 0;
    for case in &cases {
        let name = case["name"].as_str().unwrap();
        if SKIP.iter().any(|(n, _)| *n == name) {
            skipped += 1;
            continue;
        }
        let mut style = Style::default();
        for pair in case["style"].as_array().unwrap() {
            let (k, v) = (pair[0].as_str().unwrap(), pair[1].as_str().unwrap());
            set(&mut style, k, v).unwrap_or_else(|e| panic!("{name}: {e}"));
        }
        let input = case["input"].as_str().unwrap();
        let expected = case["expected"].as_str().unwrap();
        let out = match case.get("range").and_then(Value::as_array) {
            Some(r) => {
                let (start, end) = (r[0].as_u64().unwrap() as usize, r[1].as_u64().unwrap() as usize);
                cfx_lua_fmt::format_range(input, &style, start, end).map(|e| e.text)
            }
            None => cfx_lua_fmt::format(input, &style),
        };
        match out {
            Some(out) if trim(&out) == trim(expected) => passed += 1,
            Some(out) => failed.push(format!("--- {name}\n### expected\n{expected}\n### got\n{out}")),
            None => failed.push(format!("--- {name}: not formatted")),
        }
    }
    if !failed.is_empty() {
        if let Ok(path) = std::env::var("CODESTYLE_FAILS") {
            std::fs::write(path, failed.join("\n")).unwrap();
        }
        let names: Vec<&str> = failed.iter().map(|f| f.lines().next().unwrap()).collect();
        panic!("{passed} passed, {skipped} skipped, {} failed:\n{}", failed.len(), names.join("\n"));
    }
}
