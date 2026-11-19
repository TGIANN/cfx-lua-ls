//! `codestyle-check` (`cfx_lua_fmt::diagnose`). The messages and ranges were
//! checked against LuaLS 3.19.2 on 158 server files: every finding the same,
//! the only difference the end column of "must end with new line", which LuaLS
//! puts one past the end of the file.

use cfx_lua_fmt::{Style, diagnose, format};

fn found(text: &str, style: &Style) -> Vec<(String, String)> {
    diagnose(text, style).into_iter().map(|d| (text[d.start..d.start + d.len].to_string(), d.message)).collect()
}

#[test]
fn basic_findings() {
    let style = Style::default();
    let text = "local a  = \"x\"\nif a then\nprint(a)\n  end\nlocal t = {1,2}";
    let got = found(text, &style);
    // Line findings first, then the ones tied to a token, as C++ collects them.
    let want: Vec<(String, String)> = [
        ("", "expected 4 whitespace indent, found 0 whitespace"),
        ("  ", "expected 0 whitespace indent, found 2 whitespace"),
        ("", "must end with new line"),
        ("  ", "multiple spaces before token '='"),
        ("{1", "missing whitespace after token '{'"),
        (",2", "missing whitespace after token ','"),
        ("2}", "missing whitespace after token '2'"),
    ]
    .iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect();
    assert_eq!(got, want);
}

#[test]
fn formatted_text_is_clean_and_errors_are_skipped() {
    let style = Style::default();
    let text = "local function f(a, b)\n    if a then\n        return { a, b }\n    end\nend\n\nreturn f\n";
    assert_eq!(format(text, &style).as_deref(), Some(text));
    assert!(diagnose(text, &style).is_empty());
    assert!(diagnose("local x = = 1", &style).is_empty(), "a file with syntax errors is left alone");
    let mut tabs = Style::default();
    tabs.apply(&[("indent_style".to_string(), "tab".to_string())].into_iter().collect());
    let got = found("if x then\n    y()\nend\n", &tabs);
    assert_eq!(got, [("    ".to_string(), "expected 1 tab indent, found 0 tab".to_string())]);
}
