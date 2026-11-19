//! CfxLua formatter: a port of EmmyLuaCodeStyle (the formatter inside
//! lua-language-server), with its options and output.
//!
//! It builds EmmyLuaCodeStyle's own tree (`tree`) rather than reusing ours: its
//! analyzers depend on that tree's exact shape and comment placement.

pub mod editorconfig;
mod format;
pub mod style;
mod tree;

pub use editorconfig::EditorConfig;
pub use format::StyleDiagnostic;
pub use style::Style;

/// The whole file formatted, or `None` when it must be left alone: an escrow file,
/// a file with syntax errors, or one their grammar does not cover.
pub fn format(text: &str, style: &Style) -> Option<String> {
    if text.starts_with("FXAP") {
        return None;
    }
    let (bom, body) = match text.strip_prefix('\u{feff}') {
        Some(body) => ("\u{feff}", body),
        None => ("", text),
    };
    if !cfx_lua_syntax::parse(body).errors().is_empty() {
        return None;
    }
    let tree = tree::Tree::new(body)?;
    let out = format::Fmt::new(&tree, style).run();
    Some(format!("{bom}{out}"))
}

/// Lines `start_line..=end_line` (0-based) formatted: the new text and the lines it
/// replaces, from the start of the first to the end of the last with its line
/// break. The range grows to whole tokens. `None` when the file must be left alone
/// (as for [`format`]), the lines touch a `---@format disable` region, or they hold
/// no token.
pub fn format_range(text: &str, style: &Style, start_line: usize, end_line: usize) -> Option<RangeEdit> {
    if text.starts_with("FXAP") {
        return None;
    }
    let (bom, body) = match text.strip_prefix('\u{feff}') {
        Some(body) => ("\u{feff}", body),
        None => ("", text),
    };
    if !cfx_lua_syntax::parse(body).errors().is_empty() {
        return None;
    }
    let tree = tree::Tree::new(body)?;
    let (mut text, start_line, end_line) = format::Fmt::new(&tree, style).run_range(start_line, end_line)?;
    // The edit starts at column 0: keep a BOM on the first line.
    if start_line == 0 {
        text.insert_str(0, bom);
    }
    Some(RangeEdit { text, start_line, end_line })
}

/// `codestyle-check`: where the text differs from what [`format`] would write,
/// as EmmyLuaCodeStyle's `CodeStyleChecker` reports it (indentation, spaces,
/// quotes, semicolons, parentheses, the final line break). Empty when the file
/// must be left alone (as for [`format`]).
pub fn diagnose(text: &str, style: &Style) -> Vec<StyleDiagnostic> {
    if text.starts_with("FXAP") {
        return Vec::new();
    }
    let bom = if text.starts_with('\u{feff}') { '\u{feff}'.len_utf8() } else { 0 };
    let body = &text[bom..];
    if !cfx_lua_syntax::parse(body).errors().is_empty() {
        return Vec::new();
    }
    let Some(tree) = tree::Tree::new(body) else { return Vec::new() };
    let mut out = format::Fmt::new(&tree, style).diagnose();
    for d in &mut out {
        d.start += bom;
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeEdit {
    pub text: String,
    pub start_line: usize,
    pub end_line: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(text: &str) -> Option<String> {
        format(text, &Style::default())
    }

    #[test]
    fn line_ends_follow_the_file() {
        assert_eq!(fmt("local  a=1\r\nlocal b =2\r\n").unwrap(), "local a = 1\r\nlocal b = 2\r\n");
        assert_eq!(fmt("local  a=1\nlocal b =2\n").unwrap(), "local a = 1\nlocal b = 2\n");
        // Mixed or none: LF, as C++.
        assert_eq!(fmt("local a=1\r\nlocal b=2\n").unwrap(), "local a = 1\nlocal b = 2\n");
        assert_eq!(fmt("local a=1").unwrap(), "local a = 1\n");
    }

    #[test]
    fn final_newline() {
        assert_eq!(fmt("local a = 1\n\n\n").unwrap(), "local a = 1\n");
        let style = Style { insert_final_newline: false, ..Style::default() };
        assert_eq!(format("local a = 1\n\n", &style).unwrap(), "local a = 1");
    }

    #[test]
    fn bom_is_kept() {
        assert_eq!(fmt("\u{feff}local  a=1\n").unwrap(), "\u{feff}local a = 1\n");
    }

    #[test]
    fn escrow_and_broken_files_are_left_alone() {
        assert_eq!(fmt("FXAP\u{1}\u{2}"), None);
        assert_eq!(fmt("local a = (\n"), None);
    }

    #[test]
    fn long_strings_keep_trailing_spaces() {
        assert_eq!(fmt("local s = [[a  \nb]]\n").unwrap(), "local s = [[a  \nb]]\n");
        // A comment has no value to keep: its trailing spaces go.
        assert_eq!(fmt("--[[a  \nb]]\nlocal x = 1\n").unwrap(), "--[[a\nb]]\nlocal x = 1\n");
    }

    #[test]
    fn cfx_syntax_is_formatted() {
        assert_eq!(
            fmt("local a,b in t\nx={.a,.b=2}\nc+=1\nf?.g()\n").unwrap(),
            "local a, b in t\nx = { .a, .b = 2 }\nc += 1\nf?.g()\n"
        );
    }
}
