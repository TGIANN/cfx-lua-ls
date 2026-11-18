//! Folding ranges, ported from LuaLS `script/core/folding.lua` and the line rule of
//! its `textDocument/foldingRange` provider.
//!
//! Folded: functions, `do`, each `if` / `elseif` / `else` clause, `for`, `while`,
//! `repeat ... until`, table constructors, strings over several lines, long comments
//! (`--[[ ]]`, `/* */`) and `--region` ... `--endregion` pairs.
//!
//! LuaLS also folds the LuaCATS block bound to a function, `---@class` or
//! `---@alias`. Doc comments are not parsed before phase 5, so every run of two or
//! more `---` lines folds instead.

use crate::util::{if_clauses, statement_function, token, trimmed};
use crate::{Parse, TextRange, TextSize};
use cfx_lua_syntax::SyntaxKind::*;
use cfx_lua_syntax::SyntaxToken;
use rowan::NodeOrToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldKind {
    Region,
    Comment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fold {
    pub range: TextRange,
    pub kind: FoldKind,
    /// The last line folds too (comments, regions). Otherwise it stays visible:
    /// the `end` of a block, the line where the next `elseif` starts.
    pub hide_last_line: bool,
}

impl Fold {
    /// The folded lines (`line_of` maps an offset to its 0-based line), or `None`
    /// when nothing is left to fold.
    pub fn lines(&self, line_of: impl Fn(TextSize) -> u32) -> Option<(u32, u32)> {
        let start = line_of(self.range.start());
        let mut end = line_of(self.range.end());
        if !self.hide_last_line {
            end = end.saturating_sub(1);
        }
        (start < end).then_some((start, end))
    }
}

/// In source order of their start.
pub fn folding_ranges(parse: &Parse, text: &str) -> Vec<Fold> {
    let mut out = Vec::new();
    let region = |range| Fold { range, kind: FoldKind::Region, hide_last_line: false };
    let mut regions: Vec<TextSize> = Vec::new();
    let mut docs = DocRun::default();
    for element in parse.syntax().descendants_with_tokens() {
        match element {
            NodeOrToken::Node(node) => match node.kind() {
                FUNCTION_EXPR | TABLE_EXPR | DO_STAT | WHILE_STAT | NUMERIC_FOR_STAT | GENERIC_FOR_STAT => {
                    out.extend(trimmed(&node).map(region));
                }
                LOCAL_FUNCTION_STAT | FUNCTION_STAT => out.extend(statement_function(&node).map(|(r, _)| region(r))),
                IF_STAT => {
                    // The last clause runs to the end of `end`, as in LuaLS; the
                    // `end` line stays visible either way.
                    let end = trimmed(&node).map(|r| r.end());
                    let clauses = if_clauses(&node);
                    let last = clauses.len().saturating_sub(1);
                    for (i, c) in clauses.iter().enumerate() {
                        let finish = if i == last { end.unwrap_or(c.range.end()) } else { c.range.end() };
                        out.push(region(TextRange::new(c.range.start(), finish.max(c.range.start()))));
                    }
                }
                // Only a finished `repeat ... until`.
                REPEAT_STAT => {
                    if let (Some(r), Some(until)) = (trimmed(&node), token(&node, UNTIL_KW)) {
                        out.push(region(TextRange::new(r.start(), until.text_range().end())));
                    }
                }
                _ => {}
            },
            NodeOrToken::Token(t) => match t.kind() {
                // Most strings sit on one line; keep only the others.
                STRING | LONG_STRING if t.text().contains(['\n', '\r']) => out.push(region(t.text_range())),
                BLOCK_COMMENT => {
                    out.push(Fold { range: t.text_range(), kind: FoldKind::Comment, hide_last_line: false })
                }
                COMMENT => {
                    let body = t.text().strip_prefix("--").unwrap_or("").trim_start().to_ascii_lowercase();
                    if body.starts_with("region") || body.starts_with("#region") {
                        regions.push(t.text_range().start());
                    } else if (body.starts_with("endregion") || body.starts_with("#endregion"))
                        && let Some(start) = regions.pop()
                    {
                        out.push(Fold {
                            range: TextRange::new(start, t.text_range().end()),
                            kind: FoldKind::Region,
                            hide_last_line: true,
                        });
                    }
                    docs.comment(&t, text, &mut out);
                }
                _ => {}
            },
        }
    }
    docs.finish(&mut out);
    out.sort_by_key(|f| f.range.start());
    out
}

/// Consecutive `---` lines, each starting its line.
#[derive(Default)]
struct DocRun {
    /// Start of the run, end of its last line, and how many lines.
    current: Option<(TextSize, TextSize, u32)>,
}

impl DocRun {
    fn comment(&mut self, t: &SyntaxToken, text: &str, out: &mut Vec<Fold>) {
        let r = t.text_range();
        let line_start = text[..usize::from(r.start())].rfind(['\n', '\r']).map_or(0, |i| i + 1);
        let starts_line = text[line_start..usize::from(r.start())].trim().is_empty();
        if !t.text().starts_with("---") || !starts_line {
            return self.finish(out);
        }
        if let Some((start, end, lines)) = self.current {
            let between = &text[usize::from(end)..usize::from(r.start())];
            let breaks = between.matches('\n').count().max(between.matches('\r').count());
            if between.trim().is_empty() && breaks == 1 {
                self.current = Some((start, r.end(), lines + 1));
                return;
            }
            self.finish(out);
        }
        self.current = Some((r.start(), r.end(), 1));
    }

    fn finish(&mut self, out: &mut Vec<Fold>) {
        if let Some((start, end, lines)) = self.current.take()
            && lines >= 2
        {
            out.push(Fold { range: TextRange::new(start, end), kind: FoldKind::Comment, hide_last_line: true });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    /// (first line, last line, kind) as the server sends them.
    fn folds(text: &str) -> Vec<(u32, u32, FoldKind)> {
        let line_of = |o: TextSize| text[..usize::from(o)].matches('\n').count() as u32;
        folding_ranges(&parse(text), text).iter().filter_map(|f| f.lines(line_of).map(|(s, e)| (s, e, f.kind))).collect()
    }

    use FoldKind::{Comment as C, Region as R};

    #[test]
    fn blocks_keep_their_closing_line_visible() {
        let text = "\
local function f()
    return 1
end
do
    x()
end
for i = 1, 2 do
    x()
end
while x do
    x()
end
repeat
    x()
until x
";
        assert_eq!(folds(text), [(0, 1, R), (3, 4, R), (6, 7, R), (9, 10, R), (12, 13, R)]);
    }

    #[test]
    fn if_clauses_fold_one_by_one() {
        let text = "\
if a then
    x()
elseif b then
    x()
else
    x()
end
";
        assert_eq!(folds(text), [(0, 1, R), (2, 3, R), (4, 5, R)]);
    }

    #[test]
    fn tables_strings_and_nested_functions() {
        let text = "\
local t = {
    a = 1,
    f = function()
        x()
    end,
}
local s = [[
line
]]
";
        assert_eq!(folds(text), [(0, 4, R), (2, 3, R), (6, 7, R)]);
    }

    #[test]
    fn comments_and_regions_hide_their_last_line() {
        let text = "\
--[[
long
]]
/* c
style */
-- #region setup
local a = 1
-- #endregion
---@class Foo
---@field x number
local Foo = {}
local y = 1 --- trailing
--- not joined to the trailing one
";
        // `/* */` spans two lines: with its last line visible, nothing is left to fold.
        assert_eq!(folds(text), [(0, 1, C), (5, 7, R), (8, 9, C)]);
    }

    #[test]
    fn unfinished_repeat_and_single_lines_do_not_fold() {
        assert_eq!(folds("repeat\n  x()\n"), []);
        assert_eq!(folds("local t = { 1, 2 }\nlocal function f() end\n"), []);
    }
}
