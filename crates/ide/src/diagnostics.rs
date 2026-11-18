use crate::{Parse, TextRange};

/// A broken or non-Lua file (a JSON file named `.lua`, a pasted diff) can have
/// thousands of syntax errors; the first ones are the useful ones.
pub const MAX_DIAGNOSTICS: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub range: TextRange,
    /// LuaLS spelling: the error type in lower case with dashes (`miss-name`).
    pub code: String,
    pub message: String,
}

/// Syntax and compile-time errors, in source order, at most `MAX_DIAGNOSTICS`.
pub fn syntax_diagnostics(parse: &Parse) -> Vec<Diagnostic> {
    parse
        .errors()
        .iter()
        .take(MAX_DIAGNOSTICS)
        .map(|e| Diagnostic {
            range: e.range,
            code: e.code.to_ascii_lowercase().replace('_', "-"),
            message: e.message.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    #[test]
    fn codes_use_luals_spelling_and_are_capped() {
        let d = syntax_diagnostics(&parse("local = 1"));
        assert_eq!(d[0].code, "miss-name");
        let broken = "x = = \n".repeat(1000);
        assert_eq!(syntax_diagnostics(&parse(&broken)).len(), MAX_DIAGNOSTICS);
    }
}
